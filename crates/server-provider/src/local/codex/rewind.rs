use serde_json::json;

use super::{CodexClient, Transport, controls, native_sessions};
use crate::ports::agent_session::{AgentSessionError, AgentSessionSpec};
use crate::ports::native_history::SessionHistory;

impl CodexClient {
    pub(super) async fn native_rewind(
        &self,
        id: &str,
        spec: &AgentSessionSpec,
        message: &str,
    ) -> Result<SessionHistory, AgentSessionError> {
        super::validate(spec)?;
        let mut transport = Transport::spawn(&self.program, &spec.cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            rewind(&mut transport, id, spec, message).await
        }
        .await;
        transport.close().await?;
        result
    }
}

async fn rewind(
    transport: &mut Transport,
    id: &str,
    spec: &AgentSessionSpec,
    message: &str,
) -> Result<SessionHistory, AgentSessionError> {
    let source = transport
        .request("thread/read", json!({"threadId":id,"includeTurns":true}))
        .await?;
    let history = native_sessions::history(&source["thread"])?;
    if history.active
        || history.descriptor.provider_handle_id != id
        || std::fs::canonicalize(&history.descriptor.cwd).ok()
            != std::fs::canonicalize(&spec.cwd).ok()
    {
        return Err(AgentSessionError::Rejected);
    }
    let turns = source["thread"]["turns"]
        .as_array()
        .ok_or(AgentSessionError::Failed)?;
    let targets: Vec<_> = turns
        .iter()
        .enumerate()
        .filter(|(_, turn)| {
            turn["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["type"] == "userMessage" && item["id"] == message)
            })
        })
        .collect();
    if targets.len() != 1 {
        return Err(AgentSessionError::Rejected);
    }
    let index = targets[0].0;
    let (approval, sandbox, _) = controls::policy(&spec.config);
    let mut params = json!({"threadId":id,"cwd":spec.cwd,"model":spec.config.model,
        "approvalPolicy":approval,"sandbox":sandbox,"excludeTurns":false});
    if index > 0 {
        params["lastTurnId"] = turns[index - 1]["id"].clone();
    }
    let forked = transport.request("thread/fork", params).await?;
    let fork = native_sessions::text(&forked["thread"], "id")?.to_owned();
    if fork == id {
        return Err(AgentSessionError::Failed);
    }
    if index == 0 {
        let count = u32::try_from(turns.len()).map_err(|_| AgentSessionError::Failed)?;
        let rolled = transport
            .request("thread/rollback", json!({"threadId":fork,"numTurns":count}))
            .await?;
        if rolled["thread"]["id"] != fork {
            return Err(AgentSessionError::Failed);
        }
    }
    let response = transport
        .request("thread/read", json!({"threadId":fork,"includeTurns":true}))
        .await?;
    let result = native_sessions::history(&response["thread"])?;
    if result.active
        || result.descriptor.provider_handle_id != fork
        || std::fs::canonicalize(&result.descriptor.cwd).ok()
            != std::fs::canonicalize(&spec.cwd).ok()
    {
        return Err(AgentSessionError::Failed);
    }
    let expected: Vec<_> = history
        .entries
        .iter()
        .take_while(|entry| entry.turn_id.as_deref() != turns[index]["id"].as_str())
        .collect();
    if expected.len() != result.entries.len()
        || !expected.iter().zip(&result.entries).all(|(old, new)| {
            old.key == new.key && old.item == new.item && old.turn_id == new.turn_id
        })
    {
        return Err(AgentSessionError::Failed);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
