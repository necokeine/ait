use std::collections::BTreeMap;

use serde_json::{Value, json};
use server_domain::agent_runtime::{
    AgentPersistenceHandle, StoredAgentConfig, StoredAgentRuntimeInfo,
};
use uuid::Uuid;

mod commands;

use super::{ClaudeClient, config, history, permissions, streaming, transport::Transport};
use crate::ports::agent_session::{
    AgentResumePurpose, AgentSession, AgentSessionError, AgentSessionFuture, AgentSessionSpec,
    AgentTurnEvent,
};

#[derive(Debug)]
struct ClaudeSession {
    client: ClaudeClient,
    spec: AgentSessionSpec,
    id: String,
    history_only: bool,
    closed: bool,
    transport: Option<Transport>,
    active: Option<String>,
    info: StoredAgentRuntimeInfo,
    permissions: BTreeMap<String, permissions::Pending>,
    stream: streaming::Stream,
    usage: crate::local::usage::ClaudeUsage,
    output_schema: Option<Value>,
    unread_steers: std::collections::BTreeSet<String>,
    compacting: bool,
    metadata: Option<BTreeMap<String, Value>>,
    children: super::subagents::live::Live,
    notes: crate::local::notes::Notes,
    inputs: super::inputs::Inputs,
}

pub(super) async fn open(
    client: &ClaudeClient,
    spec: &AgentSessionSpec,
    resume: Option<(&AgentPersistenceHandle, AgentResumePurpose)>,
) -> Result<Box<dyn AgentSession>, AgentSessionError> {
    crate::ports::agent_session::AgentClient::validate_selection(client, spec).await?;
    config::validate_spec(spec)?;
    let (id, history_only) = match resume {
        Some((handle, purpose)) => {
            history::validate_handle(handle)?;
            if history::read(client, handle, &spec.cwd)?.is_none() {
                return Err(AgentSessionError::Unavailable);
            }
            (
                handle.session_id.clone(),
                purpose == AgentResumePurpose::History,
            )
        }
        None => (Uuid::new_v4().to_string(), false),
    };
    let mut session = ClaudeSession {
        client: client.clone(),
        spec: spec.clone(),
        id: id.clone(),
        history_only,
        closed: false,
        transport: None,
        active: None,
        permissions: BTreeMap::new(),
        stream: streaming::Stream::new(client.images.clone()),
        usage: crate::local::usage::ClaudeUsage::default(),
        output_schema: None,
        unread_steers: std::collections::BTreeSet::default(),
        compacting: false,
        children: super::subagents::live::Live::new(client.images.clone()),
        notes: crate::local::notes::Notes::restore(
            resume
                .and_then(|(handle, _)| handle.metadata.as_ref())
                .and_then(|metadata| metadata.get("controlNotes")),
        )?,
        inputs: super::inputs::Inputs::restore(
            resume
                .and_then(|(handle, _)| handle.metadata.as_ref())
                .and_then(|metadata| metadata.get("clientMessageIds")),
        )?,
        metadata: resume.map_or_else(
            || Some(super::rewind::fresh(&id, spec).resume_metadata),
            |(handle, _)| handle.metadata.clone(),
        ),
        info: StoredAgentRuntimeInfo {
            provider: "claude".to_owned(),
            session_id: Some(id),
            model: spec.config.model.clone(),
            thinking_option_id: spec.config.thinking_option_id.clone(),
            mode_id: Some(
                spec.config
                    .mode_id
                    .clone()
                    .unwrap_or_else(|| "default".to_owned()),
            ),
            extra: None,
        },
    };
    if !history_only {
        session.restore_observations()?;
        session.launch().await?;
        if resume.is_some() {
            session.children.restore(client, &session.id, &spec.cwd)?;
        }
    }
    Ok(Box::new(session))
}

impl ClaudeSession {
    fn restore_observations(&mut self) -> Result<(), AgentSessionError> {
        let path =
            history::project_dir(&self.client, &self.spec.cwd)?.join(format!("{}.jsonl", self.id));
        let records = match history::read_records(&path) {
            Ok(records) => records,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(AgentSessionError::Failed),
        };
        for record in &records {
            if record["isSidechain"] == true || record["parent_tool_use_id"].is_string() {
                continue;
            }
            self.usage.observe(record);
            self.stream.tasks.observe(record)?;
        }
        if let Some(saved) = self
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("lastUsage"))
        {
            self.usage = crate::local::usage::ClaudeUsage::restore(Some(saved))?;
        }
        if let Some(saved) = self.usage.saved() {
            self.info
                .extra
                .get_or_insert_with(BTreeMap::new)
                .insert("lastUsage".to_owned(), saved);
        }
        Ok(())
    }

    async fn launch(&mut self) -> Result<(), AgentSessionError> {
        let handle = self.handle();
        // Reading validates the cwd as well as the identity before claiming a native writer.
        if history::read(&self.client, &handle, &self.spec.cwd)?.is_none() {
            return Err(AgentSessionError::Unavailable);
        }
        let existing = history::project_dir(&self.client, &self.spec.cwd)?
            .join(format!("{}.jsonl", self.id))
            .is_file();
        if existing && let Some(metadata) = &mut self.metadata {
            metadata.remove(super::rewind::FRESH);
        }
        let mut transport = Transport::spawn(
            &self.client,
            &self.spec,
            Some((&self.id, existing)),
            self.output_schema.as_ref(),
        )?;
        let initialized = match transport.initialize().await {
            Ok(initialized) => initialized,
            Err(error) => {
                let _ = transport.close().await;
                return Err(error);
            }
        };
        let selected = self.spec.config.model.as_deref().unwrap_or("default");
        self.client.remember_models(&initialized)?;
        self.info.model = initialized["models"]
            .as_array()
            .and_then(|models| models.iter().find(|model| model["value"] == selected))
            .and_then(|model| model["resolvedModel"].as_str())
            .map(str::to_owned)
            .or_else(|| self.spec.config.model.clone());
        self.transport = Some(transport);
        Ok(())
    }

    fn handle(&self) -> AgentPersistenceHandle {
        let mut metadata = self.metadata.clone().unwrap_or_default();
        if let Some(saved) = self.usage.saved() {
            metadata.insert("lastUsage".to_owned(), saved);
        }
        if let Some(saved) = self.inputs.saved() {
            metadata.insert("clientMessageIds".to_owned(), saved);
        }
        if let Some(saved) = self.notes.saved() {
            metadata.insert("controlNotes".to_owned(), saved);
        }
        AgentPersistenceHandle {
            provider: "claude".to_owned(),
            session_id: self.id.clone(),
            native_handle: None,
            metadata: (!metadata.is_empty()).then_some(metadata),
        }
    }

    fn consume(&mut self, message: &Value) -> Result<(), AgentSessionError> {
        self.capture_identity(message)?;
        self.stream
            .events
            .extend(self.children.observe(message, &self.id, &self.spec.cwd)?);
        if message["parent_tool_use_id"].is_string()
            && matches!(
                message["type"].as_str(),
                Some("assistant" | "user" | "stream_event" | "result")
            )
        {
            return Ok(());
        }
        self.adopt_autonomous(message);
        if let Some(usage) = self.usage.observe(message) {
            self.stream.events.push_back(AgentTurnEvent::Usage(usage));
        }
        self.observe_input(message);
        match message["type"].as_str() {
            Some("control_request") => {
                if self.closed || self.history_only || self.permissions.len() >= 32 {
                    return Err(AgentSessionError::Failed);
                }
                let pending = permissions::capture(message)?;
                if self
                    .permissions
                    .values()
                    .any(|old| old.native_id == pending.native_id)
                {
                    return Err(AgentSessionError::Failed);
                }
                let id = config::text(&pending.request, "id")?.to_owned();
                self.stream
                    .events
                    .push_back(AgentTurnEvent::PermissionRequested(pending.request.clone()));
                self.permissions.insert(id, pending);
            }
            Some("control_cancel_request") => self.withdraw_permission(message)?,
            Some("system") if message["subtype"] == "init" => {
                if let Some(model) = message["model"].as_str() {
                    self.info.model = Some(model.to_owned());
                    self.stream
                        .events
                        .push_back(AgentTurnEvent::RuntimeInfo(self.info.clone()));
                }
            }
            Some("result") if self.active.is_some() => {
                if self.stream.events.iter().any(|event| {
                    matches!(
                        event,
                        AgentTurnEvent::Completed(_)
                            | AgentTurnEvent::Failed
                            | AgentTurnEvent::Cancelled
                    )
                }) {
                    return Ok(());
                }
                let success = message["subtype"] == "success"
                    && message["is_error"] != true
                    && self
                        .permissions
                        .values()
                        .all(|pending| pending.agent_id.is_some())
                    && self.stream.tools.is_empty();
                let result = if success {
                    // A previous result may already be in flight when a steer enters stdin.
                    // Its native echo/lifecycle must be observed before this turn can finish.
                    if !self.unread_steers.is_empty() {
                        return Ok(());
                    }
                    let pending = self.stream.events.len();
                    let text = self.stream.result_text(
                        message,
                        self.active.as_deref().ok_or(AgentSessionError::Failed)?,
                    )?;
                    for event in self.stream.events.iter().skip(pending) {
                        if let AgentTurnEvent::Timeline(entry) = event {
                            self.notes.push(entry.clone())?;
                        }
                    }
                    AgentTurnEvent::Completed(text)
                } else {
                    self.stream.events.extend(self.children.stopped(true, true));
                    AgentTurnEvent::Failed
                };
                self.stream.events.push_back(result);
            }
            Some("control_response") => return Err(AgentSessionError::Failed),
            _ if self.active.is_some()
                || matches!(
                    message["type"].as_str(),
                    Some("assistant" | "user" | "stream_event")
                ) =>
            {
                self.stream.record(message)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn withdraw_permission(&mut self, message: &Value) -> Result<(), AgentSessionError> {
        let native = config::text(message, "request_id")?;
        let removed: Vec<_> = self
            .permissions
            .iter()
            .filter(|(_, pending)| pending.native_id == native)
            .map(|(id, _)| id.clone())
            .collect();
        for id in removed {
            self.permissions.remove(&id);
            self.stream
                .events
                .push_back(AgentTurnEvent::PermissionResolved(id));
        }
        Ok(())
    }

    fn adopt_autonomous(&mut self, message: &Value) {
        if self.active.is_some()
            || self.closed
            || self.history_only
            || !matches!(message["type"].as_str(), Some("assistant" | "stream_event"))
            || self.stream.observed_assistant(message)
        {
            return;
        }
        let id = format!("autonomous:{}", Uuid::new_v4());
        self.active = Some(id.clone());
        self.usage.begin();
        self.stream.events.push_back(AgentTurnEvent::Started(id));
    }

    fn capture_identity(&mut self, message: &Value) -> Result<(), AgentSessionError> {
        if message
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
        {
            return Ok(());
        }
        let Some(id) = message["session_id"].as_str().filter(|id| *id != self.id) else {
            return Ok(());
        };
        if Uuid::parse_str(id).is_err()
            || !(message["type"] == "system" && message["subtype"] == "init"
                || matches!(message["type"].as_str(), Some("assistant" | "result")))
        {
            return Err(AgentSessionError::Failed);
        }
        let old = std::mem::replace(&mut self.id, id.to_owned());
        self.info.session_id = Some(self.id.clone());
        if let Some(metadata) = &mut self.metadata {
            metadata.remove(super::rewind::FRESH);
        }
        self.stream.events.push_back(AgentTurnEvent::Timeline(streaming::entry(&format!("session:{id}"),
            json!({"type":"notification","level":"info","message":format!("Claude changed native session: {old} -> {id}")}), message)));
        Ok(())
    }

    fn observe_input(&mut self, message: &Value) {
        let id = match message["type"].as_str() {
            Some("user") => message["uuid"].as_str(),
            Some("command_lifecycle")
                if matches!(message["state"].as_str(), Some("started" | "completed")) =>
            {
                message["command_uuid"].as_str()
            }
            _ => None,
        };
        if let Some(id) = id {
            self.unread_steers.remove(id);
        }
        if message["type"] == "system" {
            if message["subtype"] == "status" {
                self.compacting = message["status"] == "compacting";
            }
            if message["subtype"] == "compact_boundary" {
                self.compacting = false;
            }
        }
    }

    async fn submit_steer(
        &mut self,
        turn: &str,
        prompt: &crate::protocol::prompt::AgentPrompt,
    ) -> Result<(), AgentSessionError> {
        prompt.validate()?;
        // Consume already available native frames before checking the target turn; preserve every
        // display event for the manager. There is no replay after a failed/partial stdin write.
        for _ in 0..128 {
            let Some(message) = self
                .transport
                .as_mut()
                .ok_or(AgentSessionError::Rejected)?
                .poll()?
            else {
                break;
            };
            self.consume(&message)?;
        }
        if self.closed
            || self.history_only
            || self.active.as_deref() != Some(turn)
            || self.compacting
            || !self.permissions.is_empty()
            || self.unread_steers.len() >= 32
            || prompt.text.starts_with('/')
            || prompt.output_schema.is_some()
            || self.stream.events.iter().any(|event| {
                matches!(
                    event,
                    AgentTurnEvent::Completed(_)
                        | AgentTurnEvent::Cancelled
                        | AgentTurnEvent::Failed
                )
            })
        {
            return Err(AgentSessionError::Rejected);
        }
        let id = self.inputs.admit(prompt.client_message_id.as_deref())?;
        let message = json!({"type":"user","uuid":id,"session_id":self.id,"priority":"next",
            "parent_tool_use_id":null,"message":{"role":"user","content":prompt.claude_content()?}});
        self.transport
            .as_mut()
            .ok_or(AgentSessionError::Failed)?
            .send(&message)
            .await?;
        self.unread_steers.insert(id);
        self.stream.record(&message)?;
        Ok(())
    }
}

impl AgentSession for ClaudeSession {
    fn subagents(&self) -> Vec<crate::ports::controls::NativeSubagent> {
        self.children.children()
    }

    fn provider(&self) -> &'static str {
        "claude"
    }

    fn runtime_info(&mut self) -> AgentSessionFuture<'_, StoredAgentRuntimeInfo> {
        Box::pin(async { Ok(self.info.clone()) })
    }

    fn persistence(&self) -> Option<AgentPersistenceHandle> {
        Some(self.handle())
    }

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
        Box::pin(async move {
            let pending = self
                .permissions
                .get(id)
                .ok_or(AgentSessionError::Rejected)?;
            let reply = permissions::resolve(pending, response)?;
            self.transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .send(&reply)
                .await?;
            self.permissions.remove(id);
            Ok(())
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
            crate::ports::agent_session::AgentClient::validate_config(&self.client, config)?;
            prompt.validate()?;
            if self.closed || self.history_only || self.active.is_some() {
                return Err(AgentSessionError::Rejected);
            }
            if commands::is_rewind(&prompt.text) {
                return self.start_rewind(prompt).await;
            }
            if &self.spec.config != config || self.output_schema != prompt.output_schema {
                if let Some(mut transport) = self.transport.take() {
                    transport.close().await?;
                }
                self.spec.config = config.clone();
                self.output_schema.clone_from(&prompt.output_schema);
            }
            if self.transport.is_none() {
                self.launch().await?;
            }
            let id = self.inputs.admit(prompt.client_message_id.as_deref())?;
            let message = json!({"type":"user","uuid":id,"session_id":self.id,
                "parent_tool_use_id":null,"message":{"role":"user","content":prompt.claude_content()?}});
            // Once a native input write is attempted, a missing transcript is no longer an
            // intentional empty branch. Never silently recreate a lost accepted conversation.
            if let Some(metadata) = &mut self.metadata {
                metadata.remove(super::rewind::FRESH);
            }
            self.transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .send(&message)
                .await?;
            let events = std::mem::take(&mut self.stream.events);
            let tasks = std::mem::take(&mut self.stream.tasks);
            self.stream = streaming::Stream::new(self.client.images.clone());
            self.stream.events = events;
            self.stream.tasks = tasks;
            self.unread_steers.clear();
            self.compacting = false;
            self.usage.begin();
            // The CLI does not echo every submitted prompt. Use its supplied native UUID.
            self.stream.record(&message)?;
            self.active = Some(id.clone());
            self.info
                .thinking_option_id
                .clone_from(&config.thinking_option_id);
            self.info.mode_id = Some(
                config
                    .mode_id
                    .clone()
                    .unwrap_or_else(|| "default".to_owned()),
            );
            Ok(id)
        })
    }

    fn steer_turn<'a>(&'a mut self, turn: &'a str, text: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            self.submit_steer(turn, &crate::protocol::prompt::AgentPrompt::text(text))
                .await
        })
    }

    fn steer_input<'a>(
        &'a mut self,
        turn: &'a str,
        prompt: &'a crate::protocol::prompt::AgentPrompt,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(self.submit_steer(turn, prompt))
    }

    fn cancel_turn<'a>(&'a mut self, turn: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            if self.active.as_deref() != Some(turn) {
                return Err(AgentSessionError::Rejected);
            }
            let transport = self.transport.as_mut().ok_or(AgentSessionError::Failed)?;
            transport.request(json!({"subtype":"interrupt"})).await?;
            // Reap the interrupted query so stale results cannot finish a subsequent turn.
            transport.close().await?;
            self.transport = None;
            self.permissions.clear();
            self.stream
                .events
                .extend(self.children.stopped(false, true));
            self.stream.events.push_back(AgentTurnEvent::Cancelled);
            Ok(())
        })
    }

    fn poll_turn(&mut self) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        for _ in 0..128 {
            if let Some(mut event) = self.stream.events.pop_front() {
                if let AgentTurnEvent::Timeline(entry) = &mut event {
                    self.inputs.decorate(entry);
                }
                if matches!(
                    event,
                    AgentTurnEvent::Completed(_)
                        | AgentTurnEvent::Cancelled
                        | AgentTurnEvent::Failed
                ) {
                    self.active = None;
                    self.permissions
                        .retain(|_, pending| pending.agent_id.is_some());
                }
                return Ok(Some(event));
            }
            let Some(transport) = &mut self.transport else {
                return Ok(None);
            };
            let Some(message) = transport.poll()? else {
                return Ok(None);
            };
            self.consume(&message)?;
        }
        Ok(None)
    }

    fn close(&mut self) -> AgentSessionFuture<'_, ()> {
        Box::pin(async {
            self.closed = true;
            if let Some(transport) = &mut self.transport {
                transport.close().await?;
            }
            self.transport = None;
            self.active = None;
            self.permissions.clear();
            self.stream = streaming::Stream::new(self.client.images.clone());
            self.children.stopped(false, true);
            Ok(())
        })
    }
}
