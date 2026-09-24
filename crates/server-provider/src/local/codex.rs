//! Codex app-server session adapter, following Paseo's native thread/turn ownership.

mod transport;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentRuntimeInfo};

use crate::ports::agent_session::{
    AgentClient, AgentResumePurpose, AgentSession, AgentSessionError, AgentSessionFuture,
    AgentSessionSpec, AgentTurnEvent,
};
use transport::Transport;

/// A native Codex executable. Authentication remains in Codex's own environment and storage.
#[derive(Debug, Clone)]
pub struct CodexClient {
    program: PathBuf,
    deadline: Duration,
}

impl CodexClient {
    /// Use `program` directly without invoking a shell. Requests have a ten-second deadline.
    #[must_use]
    pub fn new(program: PathBuf) -> Self {
        Self {
            program,
            deadline: Duration::from_secs(10),
        }
    }

    async fn open(
        &self,
        spec: &AgentSessionSpec,
        resume: Option<(&AgentPersistenceHandle, AgentResumePurpose)>,
    ) -> Result<Box<dyn AgentSession>, AgentSessionError> {
        validate(spec)?;
        let mut transport = Transport::spawn(&self.program, &spec.cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            let history = resume.is_some_and(|(_, purpose)| purpose == AgentResumePurpose::History);
            let (method, mut params) = if let Some((handle, _)) = resume {
                if handle.provider != "codex" || handle.session_id.trim().is_empty() {
                    return Err(AgentSessionError::Failed);
                }
                if history {
                    (
                        "thread/read",
                        json!({"threadId":handle.session_id,"includeTurns":false}),
                    )
                } else {
                    ("thread/resume", json!({"threadId":handle.session_id}))
                }
            } else {
                ("thread/start", json!({}))
            };
            if !history {
                params["cwd"] = json!(spec.cwd);
                // The first slice has no approval UI. Its explicit mode cannot silently inherit
                // a more permissive local Codex configuration.
                params["approvalPolicy"] = json!("never");
                params["sandbox"] = json!("read-only");
                if let Some(model) = &spec.config.model {
                    params["model"] = json!(model);
                }
                if let Some(prompt) = &spec.config.system_prompt {
                    params["developerInstructions"] = json!(prompt);
                }
                if let Some(effort) = &spec.config.thinking_option_id {
                    params["config"] = json!({"model_reasoning_effort":effort});
                }
            }
            let response = transport.request(method, params).await?;
            let id = response
                .pointer("/thread/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(AgentSessionError::Failed)?
                .to_owned();
            if resume.is_some_and(|(handle, _)| handle.session_id != id) {
                return Err(AgentSessionError::Failed);
            }
            let info = StoredAgentRuntimeInfo {
                provider: "codex".to_owned(),
                session_id: Some(id.clone()),
                model: response
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| spec.config.model.clone()),
                thinking_option_id: spec.config.thinking_option_id.clone(),
                mode_id: Some("read-only".to_owned()),
                extra: None,
            };
            Ok((id, info, history))
        }
        .await;
        match result {
            Ok((id, info, history)) => {
                if history {
                    transport.close().await?;
                }
                Ok(Box::new(CodexSession {
                    transport: (!history).then_some(transport),
                    id,
                    info,
                    active_turn: None,
                    last_message: None,
                    history,
                }))
            }
            Err(error) => {
                let _ = transport.close().await;
                Err(error)
            }
        }
    }
}

impl AgentClient for CodexClient {
    fn provider(&self) -> &'static str {
        "codex"
    }

    fn is_available(&self) -> AgentSessionFuture<'_, bool> {
        Box::pin(async {
            Ok(if self.program.components().count() > 1 {
                self.program.is_file()
            } else {
                std::env::var_os("PATH").is_some_and(|paths| {
                    std::env::split_paths(&paths).any(|path| path.join(&self.program).is_file())
                })
            })
        })
    }

    fn create_session<'a>(
        &'a self,
        spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(self.open(spec, None))
    }

    fn resume_session<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        purpose: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(self.open(spec, Some((handle, purpose))))
    }
}

#[derive(Debug)]
struct CodexSession {
    transport: Option<Transport>,
    id: String,
    info: StoredAgentRuntimeInfo,
    active_turn: Option<String>,
    last_message: Option<String>,
    history: bool,
}

impl AgentSession for CodexSession {
    fn provider(&self) -> &'static str {
        "codex"
    }

    fn runtime_info(&mut self) -> AgentSessionFuture<'_, StoredAgentRuntimeInfo> {
        Box::pin(async { Ok(self.info.clone()) })
    }

    fn persistence(&self) -> Option<AgentPersistenceHandle> {
        Some(AgentPersistenceHandle {
            provider: "codex".to_owned(),
            session_id: self.id.clone(),
            native_handle: None,
            metadata: None,
        })
    }

    fn start_turn<'a>(&'a mut self, text: &'a str) -> AgentSessionFuture<'a, String> {
        Box::pin(async move {
            if self.history
                || self.active_turn.is_some()
                || text.trim().is_empty()
                || text.len() > 65536
            {
                return Err(AgentSessionError::Failed);
            }
            let response = self
                .transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .request(
                    "turn/start",
                    json!({"threadId":self.id,
                    "input":[{"type":"text","text":text,"text_elements":[]}],
                    "effort":self.info.thinking_option_id}),
                )
                .await?;
            let id = response
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(AgentSessionError::Failed)?
                .to_owned();
            self.active_turn = Some(id.clone());
            self.last_message = None;
            Ok(id)
        })
    }

    fn cancel_turn<'a>(&'a mut self, turn_id: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            if self.active_turn.as_deref() != Some(turn_id) {
                return Err(AgentSessionError::Failed);
            }
            self.transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .request(
                    "turn/interrupt",
                    json!({"threadId":self.id,"turnId":turn_id}),
                )
                .await?;
            Ok(())
        })
    }

    fn poll_turn(&mut self) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        let Some(transport) = &mut self.transport else {
            return Ok(None);
        };
        // Bound per-session work so an output-heavy provider cannot starve other Agents.
        for _ in 0..128 {
            let Some(message) = transport.poll()? else {
                return Ok(None);
            };
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .ok_or(AgentSessionError::Failed)?;
            if method == "server/unsupportedRequest" {
                return Err(AgentSessionError::Unavailable);
            }
            let params = &message["params"];
            if params.get("threadId").and_then(Value::as_str) != Some(&self.id) {
                continue;
            }
            let turn_id = params
                .get("turnId")
                .or_else(|| params.pointer("/turn/id"))
                .and_then(Value::as_str);
            if self.active_turn.is_none() || turn_id != self.active_turn.as_deref() {
                continue;
            }
            if method == "item/completed"
                && params.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage")
            {
                self.last_message = params
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            if method == "turn/completed" {
                self.active_turn = None;
                return Ok(Some(
                    match params.pointer("/turn/status").and_then(Value::as_str) {
                        Some("completed") => AgentTurnEvent::Completed(self.last_message.take()),
                        Some("interrupted") => AgentTurnEvent::Cancelled,
                        _ => AgentTurnEvent::Failed,
                    },
                ));
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> AgentSessionFuture<'_, ()> {
        Box::pin(async {
            if let Some(transport) = &mut self.transport {
                transport.close().await?;
            }
            self.transport = None;
            self.active_turn = None;
            Ok(())
        })
    }
}

fn validate(spec: &AgentSessionSpec) -> Result<(), AgentSessionError> {
    let config = &spec.config;
    if spec.provider != "codex"
        || !Path::new(&spec.cwd).is_absolute()
        || !Path::new(&spec.cwd).is_dir()
        || config
            .mode_id
            .as_deref()
            .is_some_and(|mode| mode != "read-only")
        || config
            .feature_values
            .as_ref()
            .is_some_and(|map| !map.is_empty())
        || config
            .provider_options
            .as_ref()
            .is_some_and(|map| !map.is_empty())
        || config
            .mcp_servers
            .as_ref()
            .is_some_and(|map| !map.is_empty())
        || config.tool_policy.is_some()
    {
        return Err(AgentSessionError::Unavailable);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
