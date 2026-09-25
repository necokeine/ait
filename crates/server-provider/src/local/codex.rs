//! Codex app-server session adapter, following Paseo's native thread/turn ownership.

pub(crate) mod controls;
mod discovery;
mod inspection;
mod native_sessions;
mod permissions;
mod rewind;
mod streaming;
mod transport;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use server_domain::agent_runtime::{
    AgentPersistenceHandle, StoredAgentConfig, StoredAgentRuntimeInfo,
};

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
        self.validate_remote(spec).await?;
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
                let (approval, sandbox, _) = controls::policy(&spec.config);
                params["approvalPolicy"] = json!(approval);
                params["sandbox"] = json!(sandbox);
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
                mode_id: Some(
                    spec.config
                        .mode_id
                        .clone()
                        .unwrap_or_else(|| "read-only".to_owned()),
                ),
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
                    cwd: spec.cwd.clone(),
                    permissions: std::collections::BTreeMap::new(),
                    stream: streaming::Stream::default(),
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
    fn settings(&self, config: &StoredAgentConfig) -> Value {
        json!({"availableModes":controls::modes(),"features":controls::features(config),
            "capabilities":{"supportsDynamicModes":true,"supportsRewindConversation":true,
                "supportsStreaming":true,"supportsReasoningStream":true}})
    }

    fn validate_selection<'a>(&'a self, spec: &'a AgentSessionSpec) -> AgentSessionFuture<'a, ()> {
        Box::pin(self.validate_remote(spec))
    }

    fn diagnostic(&self) -> AgentSessionFuture<'_, String> {
        Box::pin(self.native_diagnostic())
    }

    fn usage(&self) -> AgentSessionFuture<'_, Value> {
        Box::pin(self.native_usage())
    }

    fn commands<'a>(&'a self, spec: &'a AgentSessionSpec) -> AgentSessionFuture<'a, Vec<Value>> {
        Box::pin(async move {
            validate(spec)?;
            self.native_commands(&spec.cwd).await
        })
    }

    fn subagents<'a>(
        &'a self,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, Vec<crate::ports::controls::NativeSubagent>> {
        Box::pin(self.native_subagents(cwd))
    }

    fn rewind<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        message: &'a str,
    ) -> AgentSessionFuture<'a, crate::ports::native_history::SessionHistory> {
        Box::pin(async move {
            if handle.provider != "codex" {
                return Err(AgentSessionError::Unavailable);
            }
            self.native_rewind(&handle.session_id, spec, message).await
        })
    }

    fn provider(&self) -> &'static str {
        "codex"
    }

    fn validate_config(&self, config: &StoredAgentConfig) -> Result<(), AgentSessionError> {
        validate_config(config)
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

    fn discover<'a>(
        &'a self,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, crate::protocol::provider::Details> {
        Box::pin(self.discover_native(cwd))
    }

    fn history<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, Vec<crate::protocol::timeline::NativeItem>> {
        Box::pin(async move {
            if handle.provider != "codex" {
                return Err(AgentSessionError::Unavailable);
            }
            self.read_history(&handle.session_id, cwd).await
        })
    }

    fn create_session<'a>(
        &'a self,
        spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(self.open(spec, None))
    }

    fn list_sessions<'a>(
        &'a self,
        options: &'a crate::ports::native_history::ListOptions,
    ) -> AgentSessionFuture<'a, Vec<crate::ports::native_history::SessionDescriptor>> {
        Box::pin(self.list_native(options))
    }

    fn inspect_session<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, crate::ports::native_history::SessionHistory> {
        Box::pin(async move {
            if handle.provider != "codex" {
                return Err(AgentSessionError::Unavailable);
            }
            self.inspect_native(&handle.session_id, cwd).await
        })
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
    cwd: String,
    permissions: std::collections::BTreeMap<String, permissions::Pending>,
    stream: streaming::Stream,
}

impl AgentSession for CodexSession {
    fn pending_permissions(&self) -> Vec<Value> {
        self.permissions
            .values()
            .map(|pending| pending.request.clone())
            .collect()
    }

    fn respond_permission<'a>(
        &'a mut self,
        id: &'a str,
        response: &'a Value,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(self.answer_permission(id, response))
    }

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

    fn start_turn<'a>(
        &'a mut self,
        text: &'a str,
        config: &'a StoredAgentConfig,
    ) -> AgentSessionFuture<'a, String> {
        Box::pin(async move {
            validate_config(config)?;
            if self.history
                || self.active_turn.is_some()
                || text.trim().is_empty()
                || text.len() > 65536
            {
                return Err(AgentSessionError::Failed);
            }
            let (approval, _, sandbox) = controls::policy(config);
            let mut input = vec![json!({"type":"text","text":text,"text_elements":[]})];
            let transport = self.transport.as_mut().ok_or(AgentSessionError::Failed)?;
            if let Some(command) = text.strip_prefix('/') {
                let (name, args) = command
                    .split_once(char::is_whitespace)
                    .unwrap_or((command, ""));
                let response = transport
                    .request("skills/list", json!({"cwds":[self.cwd],"forceReload":true}))
                    .await?;
                let skill = controls::skills(&response, &self.cwd)?
                    .into_iter()
                    .find(|skill| skill.name == name)
                    .ok_or(AgentSessionError::Rejected)?;
                input = vec![json!({"type":"skill","name":skill.name,"path":skill.path})];
                if !args.trim().is_empty() {
                    input.push(json!({"type":"text","text":args,"text_elements":[]}));
                }
            }
            let response=transport.request("turn/start",json!({"threadId":self.id,"input":input,
                "model":config.model,"effort":config.thinking_option_id,"approvalPolicy":approval,
                "sandboxPolicy":sandbox,"serviceTier":if controls::fast(config){Some("fast")}else{None}})).await?;
            let id = response
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(AgentSessionError::Failed)?
                .to_owned();
            self.active_turn = Some(id.clone());
            self.info.model.clone_from(&config.model);
            self.info.mode_id = Some(
                config
                    .mode_id
                    .clone()
                    .unwrap_or_else(|| "read-only".to_owned()),
            );
            self.info
                .thinking_option_id
                .clone_from(&config.thinking_option_id);
            self.last_message = None;
            self.stream = streaming::Stream::default();
            Ok(id)
        })
    }

    fn steer_turn<'a>(&'a mut self, turn_id: &'a str, text: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            if self.history
                || self.active_turn.as_deref() != Some(turn_id)
                || text.trim().is_empty()
                || text.len() > 65536
                || text.starts_with('/')
                || !self.permissions.is_empty()
            {
                return Err(AgentSessionError::Rejected);
            }
            let response = self
                .transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .request(
                    "turn/steer",
                    json!({"threadId":self.id,"expectedTurnId":turn_id,
                    "input":[{"type":"text","text":text,"text_elements":[]}]}),
                )
                .await?;
            if response
                .get("turnId")
                .or_else(|| response.pointer("/turn/id"))
                .and_then(Value::as_str)
                != Some(turn_id)
            {
                return Err(AgentSessionError::Failed);
            }
            Ok(())
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
        if self.transport.is_none() {
            return Ok(None);
        }
        // Bound per-session work so an output-heavy provider cannot starve other Agents.
        for _ in 0..128 {
            let Some(message) = self
                .transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .poll()?
            else {
                return Ok(None);
            };
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .ok_or(AgentSessionError::Failed)?;
            if method == "server/unsupportedRequest" {
                return Err(AgentSessionError::Unavailable);
            }
            if message.get("id").is_some() {
                return self.capture_permission(&message).map(Some);
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
            if let Some(event) = self.stream.progress(method, params)? {
                return Ok(Some(event));
            }
            if method == "item/completed" && !self.stream.complete(&params["item"])? {
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
            if method == "item/completed" {
                let turn = turn_id.ok_or(AgentSessionError::Failed)?;
                if let Some(item) =
                    discovery::timeline_item(&params["item"], turn, &discovery::timestamp())?
                {
                    return Ok(Some(AgentTurnEvent::Timeline(item)));
                }
            }
            if method == "turn/completed" {
                self.active_turn = None;
                self.permissions.clear();
                self.stream = streaming::Stream::default();
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
            self.permissions.clear();
            self.stream = streaming::Stream::default();
            Ok(())
        })
    }
}

fn validate(spec: &AgentSessionSpec) -> Result<(), AgentSessionError> {
    if spec.provider != "codex"
        || !Path::new(&spec.cwd).is_absolute()
        || !Path::new(&spec.cwd).is_dir()
    {
        return Err(AgentSessionError::Unavailable);
    }
    validate_config(&spec.config)
}

fn validate_config(config: &StoredAgentConfig) -> Result<(), AgentSessionError> {
    if config.model.as_ref().is_some_and(|model| {
        model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control)
    }) || config.thinking_option_id.as_deref().is_some_and(|effort| {
        !matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh"
        )
    }) || config
        .mode_id
        .as_deref()
        .is_some_and(|mode| !matches!(mode, "read-only" | "auto" | "full-access"))
        || config.feature_values.as_ref().is_some_and(|map| {
            map.iter()
                .any(|(id, value)| id != "fast_mode" || !value.is_boolean())
        })
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
