use super::{AgentManager, AgentManagerError, AgentTurnEvent, now_timestamp};
use crate::ports::agent_session::AgentSessionError;

pub(super) fn drain(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    timeline: Option<&crate::storage::timeline::Timeline>,
    events: &server_metadata::service::session::SessionEvents,
    id: &str,
    agent: &mut super::LiveAgent,
) -> Result<(), AgentManagerError> {
    for _ in 0..128 {
        if agent.pending.is_none() {
            agent.pending = match agent.session.poll_turn() {
                Ok(event) => event,
                Err(_) => Some(AgentTurnEvent::Failed),
            };
        }
        let result = match &agent.pending {
            Some(AgentTurnEvent::RuntimeInfo(info)) => persist_info(registry, id, agent, info),
            Some(AgentTurnEvent::Started(turn)) => {
                persist_started(registry, timeline, id, agent, turn)
            }
            Some(AgentTurnEvent::Subagent(event)) => {
                super::controls::publish_subagent(timeline, agent, event)
            }
            Some(AgentTurnEvent::Usage(usage)) => {
                persist_usage(registry, timeline, id, agent, usage)
            }
            Some(AgentTurnEvent::PermissionRequested(request)) => {
                super::controls::publish_permission(registry, timeline, events, agent, request)?;
                Ok(())
            }
            Some(AgentTurnEvent::PermissionResolved(request)) => {
                super::controls::resolve_permission(registry, timeline, agent, request)?;
                Ok(())
            }
            Some(AgentTurnEvent::Progress { observation, entry }) => timeline
                .map_or(Ok(()), |timeline| {
                    timeline.progress(id, &agent.record.provider, observation, entry)
                }),
            Some(AgentTurnEvent::Timeline(entry)) => timeline.map_or(Ok(()), |timeline| {
                timeline
                    .append(id, &agent.record.provider, std::slice::from_ref(entry))
                    .map(|_| ())
            }),
            Some(
                AgentTurnEvent::Completed(_) | AgentTurnEvent::Cancelled | AgentTurnEvent::Failed,
            )
            | None => break,
        };
        match result {
            Ok(()) => {
                if let Some(AgentTurnEvent::Started(turn)) = agent.pending.take() {
                    agent.latest_turn = Some(turn.clone());
                    agent.turn = Some(turn);
                }
            }
            Err(
                server_model::ErrorCode::IdempotencyConflict
                | server_model::ErrorCode::ResourceExhausted,
            ) => {
                agent.pending = Some(AgentTurnEvent::Failed);
                break;
            }
            Err(_) => return Err(AgentManagerError::Registry),
        }
    }
    Ok(())
}

fn persist_info(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    id: &str,
    agent: &super::LiveAgent,
    info: &server_domain::agent_runtime::StoredAgentRuntimeInfo,
) -> Result<(), server_model::ErrorCode> {
    if !super::valid_runtime_info(info, agent.session.provider(), &agent.record.provider) {
        return Err(server_model::ErrorCode::InvalidMessage);
    }
    registry
        .update(id, &|record| {
            let mut next = record.clone();
            let mut info = info.clone();
            preserve_usage(&mut info, record.runtime_info.as_ref());
            next.runtime_info = Some(info);
            next
        })
        .map_err(|_| server_model::ErrorCode::AgentIo)?
        .ok_or(server_model::ErrorCode::AgentNotFound)?;
    Ok(())
}

fn persist_started(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    timeline: Option<&crate::storage::timeline::Timeline>,
    id: &str,
    agent: &super::LiveAgent,
    turn: &str,
) -> Result<(), server_model::ErrorCode> {
    use serde_json::json;
    use server_model::ErrorCode;
    if turn.is_empty()
        || turn.len() > 512
        || agent.turn.as_deref().is_some_and(|active| active != turn)
    {
        return Err(ErrorCode::InvalidMessage);
    }
    registry
        .update(id, &|current| {
            let mut next = current.clone();
            next.last_status = server_domain::agent_runtime::AgentRuntimeStatus::Running;
            next.updated_at = now_timestamp();
            next.last_activity_at = Some(next.updated_at.clone());
            next.last_error = None;
            if next.attention_reason
                == Some(server_domain::agent_runtime::AgentAttentionReason::Finished)
            {
                next.requires_attention = false;
                next.attention_reason = None;
                next.attention_timestamp = None;
            }
            next
        })
        .map_err(|_| ErrorCode::AgentIo)?
        .ok_or(ErrorCode::AgentNotFound)?;
    if let Some(timeline) = timeline {
        timeline.events().publish(
            id,
            "agent_stream",
            &json!({"agentId":id,"event":{
            "type":"turn_started","provider":agent.record.provider,"turnId":turn}}),
        );
    }
    Ok(())
}

fn persist_usage(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    timeline: Option<&crate::storage::timeline::Timeline>,
    id: &str,
    agent: &super::LiveAgent,
    usage: &crate::protocol::usage::AgentUsage,
) -> Result<(), server_model::ErrorCode> {
    use serde_json::json;
    use server_model::ErrorCode;
    if !usage.is_valid() {
        return Err(ErrorCode::ResourceExhausted);
    }
    let value = serde_json::to_value(usage).map_err(|_| ErrorCode::AgentIo)?;
    registry
        .update(id, &|current| {
            let mut next = current.clone();
            if let Some(info) = &mut next.runtime_info {
                info.extra
                    .get_or_insert_with(Default::default)
                    .insert("lastUsage".to_owned(), value.clone());
            }
            next
        })
        .map_err(|_| ErrorCode::AgentIo)?
        .ok_or(ErrorCode::AgentNotFound)?;
    if let Some(timeline) = timeline {
        timeline.events().publish(id, "agent_stream", &json!({"agentId":id,"event":{
            "type":"usage_updated","provider":agent.record.provider,"turnId":agent.turn,"usage":usage}}));
    }
    Ok(())
}

pub(super) fn preserve_usage(
    info: &mut server_domain::agent_runtime::StoredAgentRuntimeInfo,
    previous: Option<&server_domain::agent_runtime::StoredAgentRuntimeInfo>,
) {
    if let Some(usage) = previous
        .and_then(|info| info.extra.as_ref())
        .and_then(|extra| extra.get("lastUsage"))
    {
        info.extra
            .get_or_insert_with(Default::default)
            .insert("lastUsage".to_owned(), usage.clone());
    }
}

pub(super) fn attach_usage(
    event: &mut serde_json::Value,
    record: Option<&server_domain::agent_runtime::PersistedAgentRuntimeRecord>,
) {
    if let Some(usage) = record
        .and_then(|record| record.runtime_info.as_ref())
        .and_then(|info| info.extra.as_ref())
        .and_then(|extra| extra.get("lastUsage"))
    {
        event["usage"] = usage.clone();
    }
}

pub(super) fn publish_terminal(
    timeline: Option<&crate::storage::timeline::Timeline>,
    id: &str,
    agent: &super::LiveAgent,
    terminal: &AgentTurnEvent,
    committed: Option<&server_domain::agent_runtime::PersistedAgentRuntimeRecord>,
) {
    use serde_json::json;
    let Some(timeline) = timeline else {
        return;
    };
    let failed = matches!(terminal, AgentTurnEvent::Failed);
    let cancelled = matches!(terminal, AgentTurnEvent::Cancelled);
    let mut event = json!({"type":if failed {"turn_failed"} else if cancelled {
        "turn_canceled"} else {"turn_completed"}, "provider":agent.record.provider});
    if let Some(turn) = &agent.turn {
        event["turnId"] = json!(turn);
    }
    if failed {
        event["error"] = json!("Provider execution failed");
    }
    if cancelled {
        event["reason"] = json!("interrupted");
    }
    if !failed && !cancelled {
        attach_usage(&mut event, committed);
    }
    timeline.events().publish(
        id,
        "agent_stream",
        &json!({"agentId":id,"event":event,
        "timestamp":committed.map_or_else(now_timestamp, |record|record.updated_at.clone())}),
    );
}

impl AgentManager {
    /// Send text to the current native turn, or start a turn when already idle.
    /// The default `send` path remains exclusive for callers that own an entire turn.
    /// # Errors
    /// Returns invalid input, archived state, rejected admission or provider/storage failure.
    /// An uncertain admission closes the failed session; input is never automatically retried.
    pub async fn send_steering(
        &mut self,
        agent_id: &str,
        text: &str,
    ) -> Result<(), AgentManagerError> {
        self.steer_input(agent_id, &crate::protocol::prompt::AgentPrompt::text(text))
            .await
    }

    /// Admit all rich content into the active turn, or start a turn if already idle.
    /// # Errors
    /// Invalid input and definitive rejection preserve the active session; uncertainty closes it.
    pub async fn steer_input(
        &mut self,
        agent_id: &str,
        prompt: &crate::protocol::prompt::AgentPrompt,
    ) -> Result<(), AgentManagerError> {
        prompt
            .validate()
            .map_err(|_| AgentManagerError::InvalidRequest)?;
        if self.active_turn(agent_id).is_none() {
            return self.send_input(agent_id, prompt).await;
        }
        let record = self
            .registry
            .get(agent_id)
            .map_err(super::map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        if record.archived_at.is_some() {
            return Err(AgentManagerError::Busy);
        }
        let agent = self
            .live
            .get_mut(agent_id)
            .ok_or(AgentManagerError::Session)?;
        let turn = agent.turn.as_deref().ok_or(AgentManagerError::Busy)?;
        match agent.session.steer_input(turn, prompt).await {
            Ok(()) => {
                // Admission has already happened. Retry metadata separately so a failed write
                // cannot imply that resubmitting the input is safe.
                agent.pending_input_at = Some(now_timestamp());
                Ok(())
            }
            Err(AgentSessionError::Rejected) => Err(AgentManagerError::Busy),
            Err(AgentSessionError::Failed | AgentSessionError::Unavailable) => {
                agent.pending = Some(AgentTurnEvent::Failed);
                self.poll().await?;
                Err(AgentManagerError::Session)
            }
        }
    }
}

pub(super) fn persist_input(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    agent_id: &str,
    agent: &mut super::LiveAgent,
) -> Result<(), AgentManagerError> {
    let Some(now) = &agent.pending_input_at else {
        return Ok(());
    };
    registry
        .update(agent_id, &|current| {
            let mut next = current.clone();
            next.updated_at.clone_from(now);
            next.last_user_message_at = Some(now.clone());
            next.last_activity_at = Some(now.clone());
            next
        })
        .map_err(super::map_registry)?;
    agent.pending_input_at = None;
    Ok(())
}

pub(super) fn persist_handle(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    agent_id: &str,
    agent: &mut super::LiveAgent,
) -> Result<(), AgentManagerError> {
    let Some(handle) = agent.session.persistence() else {
        return Ok(());
    };
    if agent.record.persistence.as_ref() == Some(&handle) {
        return Ok(());
    }
    if handle.provider != agent.record.provider
        || handle.session_id.is_empty()
        || serde_json::to_vec(&handle)
            .map_err(|_| AgentManagerError::Session)?
            .len()
            > 256 * 1024
    {
        return Err(AgentManagerError::Session);
    }
    registry
        .update(agent_id, &|current| {
            let mut next = current.clone();
            next.persistence = Some(handle.clone());
            if let Some(info) = &mut next.runtime_info {
                info.session_id = Some(handle.session_id.clone());
            }
            next
        })
        .map_err(super::map_registry)?
        .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
    agent.record.persistence = Some(handle);
    Ok(())
}
