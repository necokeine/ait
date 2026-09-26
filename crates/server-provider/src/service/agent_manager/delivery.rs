//! Serialized message admission, native interruption and durable retry receipts.

use super::{AgentManager, AgentManagerError};
use crate::protocol::{agent_execution::ActiveTurnBehavior, prompt::AgentPrompt};
use crate::storage::timeline::inbox::Receipt;
use server_model::ErrorCode;

impl AgentManager {
    pub(crate) async fn deliver_answer(
        &mut self,
        agent: &str,
        prompt: &AgentPrompt,
    ) -> Result<(), ErrorCode> {
        if let Some(receipt) = self.existing_input(agent, prompt, "answer")? {
            match receipt {
                Receipt::Accepted => return Ok(()),
                Receipt::Uncertain => return Err(ErrorCode::AgentIo),
                Receipt::Rejected | Receipt::New => {}
            }
        }
        if self.live.get(agent).is_some_and(|live| live.exclusive)
            || self.has_pending_input(agent)?
        {
            return Err(ErrorCode::CatalogBusy);
        }
        let timeline = self.timeline.clone().ok_or(ErrorCode::AgentIo)?;
        let message = prompt
            .client_message_id
            .as_deref()
            .ok_or(ErrorCode::InvalidMessage)?;
        match timeline.claim_answer(agent, message, prompt)? {
            Receipt::Accepted => return Ok(()),
            Receipt::Uncertain => return Err(ErrorCode::AgentIo),
            Receipt::Rejected => return Err(ErrorCode::InvalidMessage),
            Receipt::New => {}
        }
        match self.steer_input(agent, prompt).await {
            Ok(()) => timeline.finish_input(agent, message, Receipt::Accepted),
            Err(AgentManagerError::Busy | AgentManagerError::InvalidRequest) => {
                timeline.finish_input(agent, message, Receipt::Rejected)?;
                Err(ErrorCode::CatalogBusy)
            }
            Err(_) => {
                timeline.finish_input(agent, message, Receipt::Uncertain)?;
                Err(ErrorCode::AgentIo)
            }
        }
    }

    pub(crate) fn claim_exclusive_turn(&mut self, agent: &str) -> Result<(), ErrorCode> {
        let live = self
            .live
            .get_mut(agent)
            .filter(|live| live.turn.is_some())
            .ok_or(ErrorCode::AgentIo)?;
        live.exclusive = true;
        Ok(())
    }

    pub(crate) fn has_pending_input(&self, agent: &str) -> Result<bool, ErrorCode> {
        if self
            .live
            .get(agent)
            .is_some_and(|live| live.session.pending_foreground())
        {
            return Ok(true);
        }
        self.timeline
            .as_ref()
            .map_or(Ok(false), |timeline| timeline.has_queued_input(agent))
    }

    pub(crate) async fn deliver(
        &mut self,
        agent: &str,
        prompt: &AgentPrompt,
        behavior: ActiveTurnBehavior,
    ) -> Result<(), ErrorCode> {
        prompt.validate().map_err(|_| ErrorCode::InvalidMessage)?;
        let record = self
            .registry
            .get(agent)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        let out_of_band = self
            .clients
            .get(&record.provider)
            .is_some_and(|client| client.handles_out_of_band(&prompt.text));
        let policy = if out_of_band {
            "answer"
        } else {
            match behavior {
                ActiveTurnBehavior::Interrupt => "interrupt",
                ActiveTurnBehavior::Steer => "steer",
            }
        };
        if let Some(receipt) = self.existing_input(agent, prompt, policy)? {
            match receipt {
                Receipt::Accepted => return Ok(()),
                Receipt::Uncertain => return Err(ErrorCode::AgentIo),
                Receipt::Rejected if !out_of_band => return Err(ErrorCode::InvalidMessage),
                Receipt::Rejected | Receipt::New => {}
            }
        }
        if record.archived_at.is_some() || self.live.get(agent).is_some_and(|live| live.exclusive) {
            return Err(ErrorCode::CatalogBusy);
        }
        if self
            .clients
            .get(&record.provider)
            .is_some_and(|client| client.handles_out_of_band(&prompt.text))
        {
            return self.deliver_command(agent, prompt).await;
        }
        let timeline = self.timeline.clone().ok_or(ErrorCode::AgentIo)?;
        let policy = match behavior {
            ActiveTurnBehavior::Interrupt => "interrupt",
            ActiveTurnBehavior::Steer => "steer",
        };
        let message = prompt
            .client_message_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let queued = timeline.has_queued_input(agent)?;
        match timeline.reserve_input(agent, &message, prompt, policy)? {
            Receipt::Accepted => return Ok(()),
            Receipt::Uncertain => return Err(ErrorCode::AgentIo),
            Receipt::Rejected => return Err(ErrorCode::InvalidMessage),
            Receipt::New => {}
        }
        if self.active_turn(agent).is_some()
            && matches!(behavior, ActiveTurnBehavior::Steer)
            && !queued
        {
            timeline.claim_input(agent, &message)?;
            match self.steer_input(agent, prompt).await {
                Ok(()) => return timeline.finish_input(agent, &message, Receipt::Accepted),
                Err(AgentManagerError::Busy) => timeline.requeue_input(agent, &message)?,
                Err(_) => {
                    timeline.finish_input(agent, &message, Receipt::Uncertain)?;
                    return Err(ErrorCode::AgentIo);
                }
            }
        }
        // A definitive rejection may mean the native turn just finished. Drain that result
        // before deciding whether interruption is still necessary.
        let _ = self.poll().await;
        if let Err(error) = self.interrupt_for_input(agent).await {
            timeline.finish_input(agent, &message, Receipt::Rejected)?;
            return Err(error);
        }
        // Native terminal acknowledgement may arrive later; the durable FIFO owns the input.
        self.dispatch_inputs(Some(agent)).await
    }

    async fn deliver_command(
        &mut self,
        agent: &str,
        prompt: &AgentPrompt,
    ) -> Result<(), ErrorCode> {
        let mut prompt = prompt.clone();
        let message = prompt
            .client_message_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let timeline = self.timeline.clone().ok_or(ErrorCode::AgentIo)?;
        match timeline.claim_answer(agent, &message, &prompt)? {
            Receipt::Accepted => return Ok(()),
            Receipt::Uncertain => return Err(ErrorCode::AgentIo),
            Receipt::Rejected => return Err(ErrorCode::InvalidMessage),
            Receipt::New => {}
        }
        if self.resume(agent).await.is_err() {
            timeline.finish_input(agent, &message, Receipt::Rejected)?;
            return Err(ErrorCode::AgentIo);
        }
        let session = self.live.get_mut(agent).ok_or(ErrorCode::AgentIo)?;
        let result = session.session.out_of_band(&prompt).await;
        let receipt = match &result {
            Ok(()) => Receipt::Accepted,
            Err(crate::ports::agent_session::AgentSessionError::Rejected) => Receipt::Rejected,
            Err(_) => Receipt::Uncertain,
        };
        timeline.finish_input(agent, &message, receipt)?;
        result.map_err(|_| ErrorCode::AgentIo)?;
        self.poll().await.map_err(|_| ErrorCode::AgentIo)
    }

    fn existing_input(
        &self,
        agent: &str,
        prompt: &AgentPrompt,
        policy: &str,
    ) -> Result<Option<Receipt>, ErrorCode> {
        match (&self.timeline, &prompt.client_message_id) {
            (Some(timeline), Some(message)) => {
                timeline.input_receipt(agent, message, prompt, policy)
            }
            _ => Ok(None),
        }
    }

    async fn interrupt_for_input(&mut self, id: &str) -> Result<(), ErrorCode> {
        let Some(agent) = self.live.get_mut(id) else {
            return Ok(());
        };
        agent
            .session
            .cancel_pending()
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        if let Some(turn) = &agent.turn {
            if agent.interruption_requested {
                return Ok(());
            }
            if agent.session.cancel_turn(turn).await.is_err() {
                agent.pending = Some(crate::ports::agent_session::AgentTurnEvent::Failed);
                return Err(ErrorCode::AgentIo);
            }
            agent.interruption_requested = true;
        }
        Ok(())
    }

    /// Submit one FIFO head per idle Agent. Claim commits precede every native write.
    pub(crate) async fn dispatch_pending_inputs(&mut self) -> Result<(), ErrorCode> {
        self.dispatch_inputs(None).await
    }

    async fn dispatch_inputs(&mut self, selected: Option<&str>) -> Result<(), ErrorCode> {
        let Some(timeline) = self.timeline.clone() else {
            return Ok(());
        };
        let mut seen = std::collections::BTreeSet::new();
        for input in timeline.queued_inputs()? {
            if selected.is_some_and(|agent| agent != input.agent) {
                continue;
            }
            if !seen.insert(input.agent.clone())
                || self.active_turn(&input.agent).is_some()
                || self
                    .live
                    .get(&input.agent)
                    .is_some_and(|live| live.session.pending_foreground())
            {
                continue;
            }
            let record = self
                .registry
                .get(&input.agent)
                .map_err(|_| ErrorCode::AgentIo)?;
            if record.is_none_or(|record| record.archived_at.is_some()) {
                timeline.cancel_inputs(&input.agent)?;
                continue;
            }
            timeline.claim_input(&input.agent, &input.message)?;
            match self.send_input(&input.agent, &input.prompt).await {
                Ok(()) => timeline.finish_input(&input.agent, &input.message, Receipt::Accepted)?,
                Err(error) => {
                    let state = if matches!(
                        error,
                        AgentManagerError::InvalidRequest
                            | AgentManagerError::Busy
                            | AgentManagerError::NotFound(_)
                    ) {
                        Receipt::Rejected
                    } else {
                        Receipt::Uncertain
                    };
                    timeline.finish_input(&input.agent, &input.message, state)?;
                    let queued = timeline.has_queued_input(&input.agent)?;
                    let now = super::now_timestamp();
                    let committed = self
                        .registry
                        .update(&input.agent, &|current| {
                            let mut next = current.clone();
                            next.last_status = if queued {
                                server_domain::agent_runtime::AgentRuntimeStatus::Running
                            } else {
                                server_domain::agent_runtime::AgentRuntimeStatus::Error
                            };
                            next.last_error = Some("Queued input could not be admitted".to_owned());
                            next.requires_attention = !queued;
                            next.attention_reason = (!queued).then_some(
                                server_domain::agent_runtime::AgentAttentionReason::Error,
                            );
                            next.attention_timestamp = (!queued).then(|| now.clone());
                            next.updated_at.clone_from(&now);
                            next
                        })
                        .map_err(|_| ErrorCode::AgentIo)?;
                    if let Some(committed) = committed {
                        if !queued && !committed.internal {
                            self.events.publish(server_metadata::protocol::session::SessionEventKind::AgentAttention,
                                &serde_json::json!({"agentId":input.agent,"reason":"error","timestamp":now}));
                            timeline.events().publish(&input.agent,"agent_stream",&serde_json::json!({"agentId":input.agent,
                                "event":{"type":"turn_failed","provider":committed.provider,"error":"Queued input could not be admitted"},"timestamp":now}));
                        }
                        if let Some(live) = self.live.get_mut(&input.agent) {
                            live.record = committed;
                        }
                    }
                    if selected.is_some() {
                        return Err(ErrorCode::AgentIo);
                    }
                }
            }
        }
        Ok(())
    }
}
