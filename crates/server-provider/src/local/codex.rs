//! Codex app-server session adapter, following Paseo's native thread/turn ownership.

mod async_questions;
mod commands;
pub(crate) mod controls;
mod discovery;
mod inspection;
mod native_sessions;
mod permissions;
mod plans;
mod prompts;
mod rewind;
mod streaming;
mod subagents;
mod transport;
mod workflows;

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
    capabilities: std::sync::Arc<std::sync::atomic::AtomicU8>,
    images: super::images::ImageStore,
}

impl CodexClient {
    /// Use `program` directly without invoking a shell. Requests have a ten-second deadline.
    #[must_use]
    pub fn new(program: PathBuf) -> Self {
        Self {
            program,
            deadline: Duration::from_secs(10),
            capabilities: std::sync::Arc::default(),
            images: super::images::ImageStore::default(),
        }
    }

    /// Store decoded native image output in a private, persistent directory.
    #[must_use]
    pub fn with_image_directory(mut self, directory: PathBuf) -> Self {
        self.images = super::images::ImageStore::new(directory);
        self
    }

    async fn restore_children(
        &self,
        restore: bool,
        cwd: &str,
        id: &str,
    ) -> Result<subagents::Live, AgentSessionError> {
        if restore {
            subagents::Live::restore(self.native_subagents(cwd).await?, id)
        } else {
            Ok(subagents::Live::default())
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
            self.inspect_workflows(&mut transport).await?;
            if self.goals() {
                transport.close().await?;
                transport =
                    Transport::spawn_with_goals(&self.program, &spec.cwd, self.deadline, true)?;
                transport.initialize().await?;
            }
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
                params["approvalsReviewer"] = json!(workflows::reviewer(&spec.config));
                if let Some(model) = &spec.config.model {
                    params["model"] = json!(model);
                }
                if let Some(prompt) = &spec.config.system_prompt {
                    params["developerInstructions"] = json!(prompt);
                }
                params["config"] = crate::local::configuration::codex(&spec.config)?;
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
            let info = runtime_info(&response, &id, spec);
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
                    id: id.clone(),
                    info,
                    active_turn: None,
                    last_message: None,
                    history,
                    cwd: spec.cwd.clone(),
                    permissions: std::collections::BTreeMap::new(),
                    questions: async_questions::Questions::restore(saved(
                        resume,
                        "asyncQuestions",
                    ))?,
                    pending_plan: plans::Plan::restore(saved(resume, "pendingPlan"))?,
                    latest_plan: None,
                    stream: streaming::Stream::default(),
                    children: self
                        .restore_children(resume.is_some() && !history, &spec.cwd, &id)
                        .await?,
                    notes: crate::local::notes::Notes::restore(saved(resume, "controlNotes"))?,
                    last_anchor: None,
                    manual_compactions: 0,
                    pending_goal_start: false,
                    config: spec.config.clone(),
                    client: self.clone(),
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
    fn handles_out_of_band(&self, text: &str) -> bool {
        commands::out_of_band(text)
    }
    fn persisted_permissions(&self, handle: &AgentPersistenceHandle) -> Vec<Value> {
        async_questions::Questions::restore(
            handle
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("asyncQuestions")),
        )
        .map(|questions| questions.pending())
        .unwrap_or_default()
        .into_iter()
        .chain(
            plans::Plan::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("pendingPlan")),
            )
            .ok()
            .flatten()
            .map(|plan| plan.request()),
        )
        .collect()
    }
    fn settings(&self, config: &StoredAgentConfig) -> Value {
        json!({"availableModes":self.modes(),"features":self.features(config),
            "capabilities":{"supportsDynamicModes":true,"supportsRewindConversation":true,"supportsMcpServers":true,
                "supportsStreaming":true,"supportsReasoningStream":true,"supportsSessionListing":true}})
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
            let mut history = self
                .native_rewind(&handle.session_id, spec, message)
                .await?;
            let mut questions = async_questions::Questions::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("asyncQuestions")),
            )?;
            let mut notes = crate::local::notes::Notes::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("controlNotes")),
            )?;
            if let Some(plan) = plans::Plan::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("pendingPlan")),
            )?
            .filter(|plan| plan.retained(&history.entries))
            {
                history
                    .resume_metadata
                    .insert("pendingPlan".to_owned(), json!(plan));
            }
            notes.retain(&history.entries);
            notes.history(&mut history.entries);
            if let Some(saved) = notes.saved() {
                history
                    .resume_metadata
                    .insert("controlNotes".to_owned(), saved);
            }
            questions.retain(&history.entries);
            questions.history(&mut history.entries)?;
            if let Some(saved) = questions.saved() {
                history
                    .resume_metadata
                    .insert("asyncQuestions".to_owned(), saved);
            }
            Ok(history)
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
            let mut entries = self.read_history(&handle.session_id, cwd).await?;
            crate::local::notes::Notes::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("controlNotes")),
            )?
            .history(&mut entries);
            async_questions::Questions::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("asyncQuestions")),
            )?
            .history(&mut entries)?;
            Ok(entries)
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
            let mut history = self.inspect_native(&handle.session_id, cwd).await?;
            if let Some(plan) = plans::Plan::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("pendingPlan")),
            )? {
                history
                    .resume_metadata
                    .insert("pendingPlan".to_owned(), json!(plan));
            }
            let notes = crate::local::notes::Notes::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("controlNotes")),
            )?;
            notes.history(&mut history.entries);
            if let Some(saved) = notes.saved() {
                history
                    .resume_metadata
                    .insert("controlNotes".to_owned(), saved);
            }
            let questions = async_questions::Questions::restore(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("asyncQuestions")),
            )?;
            questions.history(&mut history.entries)?;
            if let Some(saved) = questions.saved() {
                history
                    .resume_metadata
                    .insert("asyncQuestions".to_owned(), saved);
            }
            Ok(history)
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
    questions: async_questions::Questions,
    pending_plan: Option<plans::Plan>,
    latest_plan: Option<plans::Plan>,
    children: subagents::Live,
    notes: crate::local::notes::Notes,
    last_anchor: Option<String>,
    manual_compactions: u8,
    pending_goal_start: bool,
    stream: streaming::Stream,
    config: StoredAgentConfig,
    client: CodexClient,
}

impl CodexSession {
    fn finish_turn(&mut self, params: &Value) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        self.last_anchor.clone_from(&self.active_turn);
        self.active_turn = None;
        self.permissions
            .retain(|_, pending| pending.request["input"]["threadId"] != self.id);
        let queued = std::mem::take(&mut self.stream.events);
        self.stream = streaming::Stream::default();
        self.stream.events = queued;
        self.manual_compactions = 0;
        let terminal = match params.pointer("/turn/status").and_then(Value::as_str) {
            Some("completed") => AgentTurnEvent::Completed(self.last_message.take()),
            Some("interrupted") => AgentTurnEvent::Cancelled,
            _ => AgentTurnEvent::Failed,
        };
        if matches!(terminal, AgentTurnEvent::Cancelled) {
            for request in self.questions.pending() {
                let id = request["id"].as_str().ok_or(AgentSessionError::Failed)?;
                let entry = self.questions.resolve(id, &json!({"behavior":"deny"}))?;
                self.stream
                    .events
                    .push_back(AgentTurnEvent::Timeline(entry));
                self.stream
                    .events
                    .push_back(AgentTurnEvent::PermissionResolved(id.to_owned()));
            }
        }
        self.finish_plan(matches!(terminal, AgentTurnEvent::Completed(_)))?;
        self.stream.events.push_back(terminal);
        Ok(self.stream.events.pop_front())
    }

    fn observe_interaction(&mut self, item: &Value, turn: &str) -> Result<(), AgentSessionError> {
        if item["type"] == "plan"
            && self
                .config
                .feature_values
                .as_ref()
                .is_some_and(|features| features.get("plan_mode") == Some(&json!(true)))
        {
            self.latest_plan = plans::Plan::receive(item, turn)?;
        }
        if item["delivery"] == "async"
            && let Some(request) = self.questions.receive(item)?
        {
            self.stream
                .events
                .push_back(AgentTurnEvent::PermissionRequested(request));
        }
        Ok(())
    }

    async fn apply_configuration(
        &mut self,
        config: &StoredAgentConfig,
    ) -> Result<(), AgentSessionError> {
        if self.config.provider_options == config.provider_options
            && self.config.mcp_servers == config.mcp_servers
            && self.config.tool_policy == config.tool_policy
            && self.config.system_prompt == config.system_prompt
        {
            return Ok(());
        }
        if let Some(mut transport) = self.transport.take() {
            transport.close().await?;
        }
        // Thread-level MCP/config changes require a new native process; resuming an
        // already-loaded thread can return its cached configuration unchanged.
        let mut transport = Transport::spawn_with_goals(
            &self.client.program,
            &self.cwd,
            self.client.deadline,
            self.client.goals(),
        )?;
        let result = async {
            transport.initialize().await?;
            let (approval, sandbox, _) = controls::policy(config);
            let response = transport
                .request(
                    "thread/resume",
                    json!({"threadId":self.id,
                "cwd":self.cwd,"model":config.model,"approvalPolicy":approval,"sandbox":sandbox,
                "config":crate::local::configuration::codex(config)?,
                "approvalsReviewer":workflows::reviewer(config),
                "developerInstructions":config.system_prompt}),
                )
                .await?;
            if response["thread"]["id"] != self.id {
                return Err(AgentSessionError::Failed);
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = transport.close().await;
            return Err(error);
        }
        self.transport = Some(transport);
        Ok(())
    }
}

impl AgentSession for CodexSession {
    fn pending_foreground(&self) -> bool {
        self.pending_goal_start || self.manual_compactions > 0
    }

    fn cancel_pending(&mut self) -> AgentSessionFuture<'_, ()> {
        Box::pin(async {
            if self.pending_goal_start {
                self.transport
                    .as_mut()
                    .ok_or(AgentSessionError::Failed)?
                    .request(
                        "thread/goal/set",
                        json!({"threadId":self.id,"status":"paused"}),
                    )
                    .await?;
                self.pending_goal_start = false;
            }
            Ok(())
        })
    }

    fn out_of_band<'a>(
        &'a mut self,
        prompt: &'a crate::protocol::prompt::AgentPrompt,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(self.execute_command(prompt))
    }
    fn subagents(&self) -> Vec<crate::ports::controls::NativeSubagent> {
        self.children.children()
    }
    fn prepare_permission_response(
        &self,
        id: &str,
        response: &Value,
    ) -> Result<Option<crate::protocol::prompt::AgentPrompt>, AgentSessionError> {
        if self.questions.contains(id) {
            self.questions.prepare(id, response)
        } else if let Some(plan) = self
            .pending_plan
            .as_ref()
            .filter(|plan| plan.request_id() == id)
        {
            let mut notes = self.notes.clone();
            notes.push(plan.resolution(response)?)?;
            plan.prepare(response)
        } else {
            Ok(None)
        }
    }

    fn permission_config_patch(
        &self,
        id: &str,
        response: &Value,
    ) -> Result<Option<crate::protocol::agent_config::ConfigPatch>, AgentSessionError> {
        self.plan_patch(id, response)
    }

    fn pending_permissions(&self) -> Vec<Value> {
        self.permissions
            .values()
            .map(|pending| pending.request.clone())
            .chain(self.questions.pending())
            .chain(self.pending_plan.as_ref().map(plans::Plan::request))
            .collect()
    }

    fn respond_permission<'a>(
        &'a mut self,
        id: &'a str,
        response: &'a Value,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            if self.questions.contains(id) {
                let entry = self.questions.resolve(id, response)?;
                self.stream
                    .events
                    .push_back(AgentTurnEvent::Timeline(entry));
                return Ok(());
            }
            if self
                .pending_plan
                .as_ref()
                .is_some_and(|plan| plan.request_id() == id)
            {
                return self.resolve_plan(response);
            }
            self.answer_permission(id, response).await
        })
    }

    fn provider(&self) -> &'static str {
        "codex"
    }

    fn runtime_info(&mut self) -> AgentSessionFuture<'_, StoredAgentRuntimeInfo> {
        Box::pin(async { Ok(self.info.clone()) })
    }

    fn persistence(&self) -> Option<AgentPersistenceHandle> {
        let mut metadata = std::collections::BTreeMap::new();
        if let Some(plan) = &self.pending_plan {
            metadata.insert("pendingPlan".to_owned(), json!(plan));
        }
        if let Some(saved) = self.questions.saved() {
            metadata.insert("asyncQuestions".to_owned(), saved);
        }
        if let Some(saved) = self.notes.saved() {
            metadata.insert("controlNotes".to_owned(), saved);
        }
        Some(AgentPersistenceHandle {
            provider: "codex".to_owned(),
            session_id: self.id.clone(),
            native_handle: None,
            metadata: (!metadata.is_empty()).then_some(metadata),
        })
    }

    fn start_turn<'a>(
        &'a mut self,
        text: &'a str,
        config: &'a StoredAgentConfig,
    ) -> AgentSessionFuture<'a, String> {
        Box::pin(async move {
            self.start_input(&crate::protocol::prompt::AgentPrompt::text(text), config)
                .await
        })
    }

    fn start_input<'a>(
        &'a mut self,
        prompt: &'a crate::protocol::prompt::AgentPrompt,
        config: &'a StoredAgentConfig,
    ) -> AgentSessionFuture<'a, String> {
        Box::pin(async move {
            validate_config(config)?;
            prompt.validate()?;
            if self.history || self.active_turn.is_some() {
                return Err(AgentSessionError::Failed);
            }
            self.preflight_plan(prompt)?;
            self.apply_configuration(config).await?;
            let (approval, _, sandbox) = controls::policy(config);
            let transport = self.transport.as_mut().ok_or(AgentSessionError::Failed)?;
            let input = commands::input(transport, &self.cwd, prompt).await?;
            let collaboration = workflows::collaboration(
                transport,
                config,
                self.info.model.as_deref(),
                self.config
                    .feature_values
                    .as_ref()
                    .is_some_and(|values| values.contains_key("plan_mode")),
            )
            .await?;
            let mut params = json!({"threadId":self.id,"input":input,
                "model":config.model,"effort":config.thinking_option_id,"approvalPolicy":approval,
                "clientUserMessageId":prompt.client_message_id,"outputSchema":prompt.output_schema,
                "approvalsReviewer":workflows::reviewer(config),
                "sandboxPolicy":sandbox,"serviceTier":if controls::fast(config){Some("fast")}else{None}});
            if let Some(collaboration) = collaboration {
                params["collaborationMode"] = collaboration;
            }
            let response = transport.request("turn/start", params).await?;
            let id = response
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(AgentSessionError::Failed)?
                .to_owned();
            self.dismiss_plan_for(prompt)?;
            self.latest_plan = None;
            self.active_turn = Some(id.clone());
            self.config = config.clone();
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
            let queued = std::mem::take(&mut self.stream.events);
            self.stream = streaming::Stream::default();
            self.stream.events = queued;
            Ok(id)
        })
    }

    fn steer_turn<'a>(&'a mut self, turn_id: &'a str, text: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            self.steer_input(turn_id, &crate::protocol::prompt::AgentPrompt::text(text))
                .await
        })
    }

    fn steer_input<'a>(
        &'a mut self,
        turn_id: &'a str,
        prompt: &'a crate::protocol::prompt::AgentPrompt,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            prompt.validate()?;
            if self.history
                || self.active_turn.as_deref() != Some(turn_id)
                || prompt.text.starts_with('/')
                || prompt.output_schema.is_some()
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
                    "clientUserMessageId":prompt.client_message_id,"input":prompt.codex_input()?}),
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
            if let Some(event) = self.stream.events.pop_front() {
                return Ok(Some(event));
            }
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
            self.stream.events.extend(self.children.observe(
                method,
                params,
                (&self.id, &self.cwd),
                &self.client.images,
            )?);
            if method == "serverRequest/resolved"
                && params["threadId"]
                    .as_str()
                    .is_some_and(|id| id == self.id || self.children.contains(id))
            {
                if let Some(id) = self.resolve_native_permission(&params["requestId"]) {
                    return Ok(Some(AgentTurnEvent::PermissionResolved(id)));
                }
                continue;
            }
            if params.get("threadId").and_then(Value::as_str) != Some(&self.id) {
                continue;
            }
            if let Some(event) = self.control_event(method, params)? {
                return Ok(Some(event));
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
                self.observe_interaction(&params["item"], turn)?;
                let entries = discovery::timeline_items(
                    &params["item"],
                    turn,
                    &discovery::timestamp(),
                    &self.client.images,
                )?;
                self.stream
                    .events
                    .extend(entries.into_iter().map(AgentTurnEvent::Timeline));
                if let Some(event) = self.stream.events.pop_front() {
                    return Ok(Some(event));
                }
            }
            if method == "turn/completed" {
                return self.finish_turn(params);
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
            self.children.stopped();
            self.stream = streaming::Stream::default();
            Ok(())
        })
    }
}

fn runtime_info(response: &Value, id: &str, spec: &AgentSessionSpec) -> StoredAgentRuntimeInfo {
    StoredAgentRuntimeInfo {
        provider: "codex".to_owned(),
        session_id: Some(id.to_owned()),
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
        .is_some_and(|mode| !matches!(mode, "read-only" | "auto" | "auto-review" | "full-access"))
        || config.feature_values.as_ref().is_some_and(|map| {
            map.iter().any(|(id, value)| {
                !matches!(id.as_str(), "fast_mode" | "plan_mode") || !value.is_boolean()
            })
        })
    {
        return Err(AgentSessionError::Unavailable);
    }
    crate::local::configuration::validate(config, "codex")
}

#[cfg(all(test, unix))]
mod tests;

fn saved<'a>(
    resume: Option<(&'a AgentPersistenceHandle, AgentResumePurpose)>,
    key: &str,
) -> Option<&'a Value> {
    resume
        .and_then(|(handle, _)| handle.metadata.as_ref())
        .and_then(|metadata| metadata.get(key))
}
