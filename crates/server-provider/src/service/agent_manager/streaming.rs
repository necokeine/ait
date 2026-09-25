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
            Some(AgentTurnEvent::PermissionRequested(request)) => {
                super::controls::publish_permission(registry, timeline, events, agent, request)?;
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
            Ok(()) => agent.pending = None,
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
        if text.trim().is_empty() || text.len() > 65536 {
            return Err(AgentManagerError::InvalidRequest);
        }
        if self.active_turn(agent_id).is_none() {
            return self.send(agent_id, text).await;
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
        match agent.session.steer_turn(turn, text).await {
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
