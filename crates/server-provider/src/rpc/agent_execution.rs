//! Provider worker request decoding and response projections.

mod controls;
mod native_sessions;
mod voice;

use std::path::Path;

use serde_json::{Value, json};
use server_domain::agent_runtime::PersistedAgentRuntimeRecord;
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use uuid::Uuid;

use crate::ports::agent_runtime::AgentRuntimeRegistry;
use crate::ports::agent_session::AgentSessionSpec;
use crate::protocol::agent_execution::{CreateRequest, ResumeRequest, SendRequest};
use crate::protocol::agent_lifecycle::AgentIdRequest;
use crate::rpc::ErrorCode;
use crate::service::agent_manager::{AgentManager, AgentManagerError, AgentRegistration};
use crate::service::agent_runtime::AgentRuntimeDirectory;

pub(crate) struct ExecutionState {
    pub(crate) manager: AgentManager,
    pub(crate) directory: AgentRuntimeDirectory,
    pub(crate) registry: Box<dyn AgentRuntimeRegistry>,
    pub(crate) workspaces: Box<dyn WorkspaceRegistry>,
    pub(crate) projects: Box<dyn ProjectRegistry>,
    pub(crate) import_directory: Option<server_metadata::service::directory::Directory>,
}

impl ExecutionState {
    pub(crate) async fn execute(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, ErrorCode> {
        self.manager
            .poll()
            .await
            .map_err(|error| map_manager(&error))?;
        match method {
            "internal.voice.send" | "internal.voice.status" | "internal.voice.cancel" => {
                self.voice(method, params).await
            }
            "agent.rewind.request"
            | "agent.commands.list.request"
            | "agent.permission.resolve.request"
            | "agent.provider_subagents.list.request"
            | "agent.provider_subagents.timeline.get.request"
            | "provider.diagnostic.request"
            | "provider.usage.list.request" => self.controls(method, params).await,
            "provider.sessions.recent.list.request"
            | "agent.import.request"
            | "agent.refresh.request"
            | "agent.fork_context.request" => self.native_sessions(method, params).await,
            "agent.create.request" => self.create(params).await,
            "provider.available.list.request"
            | "provider.models.list.request"
            | "provider.modes.list.request"
            | "provider.features.list.request"
            | "provider.snapshot.get.request"
            | "provider.snapshot.refresh.request" => self
                .manager
                .providers(method, params)
                .await
                .map_err(Into::into),
            "agent.timeline.get.request"
            | "agent.timeline.search.request"
            | "agent.timeline.list_prompts.request"
            | "internal.timeline.append" => self.timeline(method, params).await,
            "agent.resume.request" => self.resume(params).await,
            "agent.message.send.request" => self.send(params).await,
            "agent.model.set.request"
            | "agent.thinking.set.request"
            | "agent.config.apply.request"
            | "agent.mode.set.request"
            | "agent.feature.set.request" => self.configure(method, &params).await,
            "agent.cancel.request" => {
                only(&params, &["agentId"])?;
                let request: AgentIdRequest = decode(params)?;
                let id = self.resolve(&request.agent_id)?;
                self.manager
                    .cancel(&id)
                    .await
                    .map_err(|error| map_manager(&error))?;
                Ok(json!({"agentId":id,"agent":self.snapshot(&id)?,"error":null}))
            }
            "agent.finish.wait.request" => {
                let request: AgentIdRequest = decode(params)?;
                let id = self.resolve(&request.agent_id)?;
                let snapshot = self.snapshot(&id)?;
                let status = if self.manager.active_turn(&id).is_some() {
                    "running"
                } else if snapshot["status"] == "error" || snapshot["status"] == "running" {
                    "error"
                } else {
                    "idle"
                };
                Ok(json!({"status":status,"final":snapshot,
                    "error": if status == "error" { Some("Provider execution failed") } else { None },
                    "lastMessage":self.manager.last_message(&id)}))
            }
            _ => {
                let result = super::agent_runtime::execute(&mut self.directory, method, params);
                // Archive may cascade and can partially succeed. Reconcile even on RPC failure.
                self.manager
                    .reconcile()
                    .await
                    .map_err(|error| map_manager(&error))?;
                let mut value = result?;
                self.decorate(&mut value)?;
                Ok(value)
            }
        }
    }

    async fn timeline(&mut self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        use crate::protocol::timeline::{AppendRequest, FetchRequest, SearchRequest};
        let plugin = params
            .get("plugin")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let params = if method == "internal.timeline.append" {
            params["request"].clone()
        } else {
            params
        };
        let identifier = params["agentId"]
            .as_str()
            .ok_or(ErrorCode::InvalidMessage)?;
        let id = self.resolve(identifier)?;
        self.manager.load_timeline(&id).await?;
        let timeline = self
            .manager
            .timeline()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        match method {
            "agent.timeline.get.request" => {
                let mut request: FetchRequest = decode(params)?;
                request.agent_id.clone_from(&id);
                let (epoch, rows) = timeline.read(&id)?;
                super::timeline::fetch(&request, &epoch, &rows, &self.snapshot(&id)?)
                    .map_err(Into::into)
            }
            "agent.timeline.search.request" => {
                let mut request: SearchRequest = decode(params)?;
                request.agent_id = id;
                let (epoch, rows) = timeline.read(&request.agent_id)?;
                super::timeline::search(&request, &epoch, &rows).map_err(Into::into)
            }
            "agent.timeline.list_prompts.request" => {
                only(&params, &["agentId"])?;
                let (epoch, rows) = timeline.read(&id)?;
                super::timeline::prompts(&id, &epoch, &rows).map_err(Into::into)
            }
            "internal.timeline.append" => {
                let plugin = plugin.ok_or(ErrorCode::UnsupportedCapability)?;
                let request: AppendRequest = decode(params)?;
                if request.item.r#type != "plugin"
                    || request.item.version == 0
                    || !server_model::valid_id(&request.item.id)
                    || !server_model::valid_id(&request.item.kind)
                    || serde_json::to_vec(&request.item.data)
                        .map_err(|_| ErrorCode::InvalidMessage)?
                        .len()
                        > 65536
                {
                    return Err(ErrorCode::InvalidMessage);
                }
                let key = format!("plugin:{plugin}:{}", request.item.id);
                let mut item =
                    serde_json::to_value(request.item).map_err(|_| ErrorCode::InvalidMessage)?;
                item["pluginId"] = json!(plugin);
                let entry = crate::protocol::timeline::NativeItem {
                    key,
                    turn_id: None,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    item,
                };
                let provider = self
                    .registry
                    .get(&id)
                    .map_err(|_| ErrorCode::AgentIo)?
                    .ok_or(ErrorCode::AgentNotFound)?
                    .provider;
                let (epoch, positions) = timeline.append(&id, &provider, &[entry])?;
                Ok(json!({"epoch":epoch,"seq":positions.first()}))
            }
            _ => Err(ErrorCode::MethodNotFound),
        }
    }

    async fn create(&mut self, params: Value) -> Result<Value, ErrorCode> {
        let (request, intent) = parse_creation(params)?;
        let workspace_id = self.workspace(request.workspace_id.as_deref(), &request.config.cwd)?;
        if let Some(id) = &request.agent_id {
            Uuid::parse_str(id).map_err(|_| ErrorCode::InvalidMessage)?;
        }
        let key = request
            .idempotency_key
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let creations = self.manager.creations();
        let admission = creations.begin(
            server_metadata::protocol::creation::Kind::Agent,
            &key,
            intent,
        )?;
        if !admission.execute {
            return Ok(
                json!({"agentId":admission.snapshot.agent_id,"agent":admission.snapshot.agent,"error":admission.snapshot.error,"creation":admission.snapshot}),
            );
        }
        let id = match admission.snapshot.agent_id.clone() {
            Some(id) => {
                Uuid::parse_str(&id).map_err(|_| ErrorCode::InvalidMessage)?;
                id
            }
            None => Uuid::new_v4().to_string(),
        };
        let created = self
            .manager
            .create(
                &id,
                &AgentSessionSpec {
                    provider: request.config.provider,
                    cwd: request.config.cwd,
                    config: request.config.stored,
                },
                AgentRegistration {
                    workspace_id: Some(workspace_id),
                    title: request.config.title.map(|title| title.trim().to_owned()),
                    labels: request.labels,
                    internal: false,
                },
            )
            .await;
        if let Err(error) = created {
            creations.advance(&admission.snapshot, "failed", None, Some(error.to_string()))?;
            return Err(map_manager(&error));
        }
        let result = json!({"agent":self.snapshot(&id)?});
        let mut progress =
            creations.advance(&admission.snapshot, "agent_ready", Some(result), None)?;
        if let Some(prompt) = request.initial_prompt {
            if let Err(error) = self.manager.send(&id, &prompt).await {
                creations.advance(&progress, "failed", None, Some(error.to_string()))?;
                return Err(map_manager(&error));
            }
            progress = creations.advance(&progress, "prompt_started", None, None)?;
        }
        let snapshot = self.snapshot(&id)?;
        progress = creations.advance(
            &progress,
            "completed",
            Some(json!({"agent":snapshot})),
            None,
        )?;
        Ok(
            json!({"status":"agent_created","agentId":id,"agent":snapshot,"error":null,"creation":progress}),
        )
    }

    async fn resume(&mut self, params: Value) -> Result<Value, ErrorCode> {
        only(&params, &["handle"])?;
        let request: ResumeRequest = decode(params)?;
        let records = self.registry.list().map_err(|_| ErrorCode::AgentIo)?;
        let mut matching = records.iter().filter(|record| {
            record.persistence.as_ref().is_some_and(|handle| {
                handle.provider == request.handle.provider
                    && handle.session_id == request.handle.session_id
            })
        });
        let record = matching.next().ok_or(ErrorCode::AgentNotFound)?;
        if matching.next().is_some() {
            return Err(ErrorCode::InvalidMessage);
        }
        if record.archived_at.is_none() {
            self.workspace(record.workspace_id.as_deref(), &record.cwd)?;
        }
        self.manager
            .resume(&record.id)
            .await
            .map_err(|error| map_manager(&error))?;
        Ok(json!({"status":"agent_resumed","agentId":record.id,"agent":self.snapshot(&record.id)?}))
    }

    async fn send(&mut self, params: Value) -> Result<Value, ErrorCode> {
        only(&params, &["agentId", "text", "activeTurnBehavior"])?;
        let request: SendRequest = decode(params)?;
        let id = self.resolve(&request.agent_id)?;
        let record = self
            .registry
            .get(&id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        self.workspace(record.workspace_id.as_deref(), &record.cwd)?;
        let result = if request.active_turn_behavior.is_some() {
            self.manager.send_steering(&id, &request.text).await
        } else {
            self.manager.send(&id, &request.text).await
        };
        Ok(
            json!({"agentId":id,"accepted":result.is_ok(),"error":result.err().map(|error| error.to_string())}),
        )
    }

    fn workspace(&self, selected: Option<&str>, cwd: &str) -> Result<String, ErrorCode> {
        let canonical = std::fs::canonicalize(cwd).map_err(|_| ErrorCode::InvalidMessage)?;
        let workspace = if let Some(id) = selected {
            self.workspaces.get(id).map_err(|_| ErrorCode::RegistryIo)?
        } else {
            self.workspaces
                .list()
                .map_err(|_| ErrorCode::RegistryIo)?
                .into_iter()
                .filter(|workspace| {
                    workspace.archived_at.as_ref().is_none_or(String::is_empty)
                        && std::fs::canonicalize(&workspace.cwd).ok().as_ref() == Some(&canonical)
                })
                .min_by(|left, right| {
                    left.created_at
                        .cmp(&right.created_at)
                        .then(left.workspace_id.cmp(&right.workspace_id))
                })
        }
        .ok_or(ErrorCode::InvalidMessage)?;
        let project = self
            .projects
            .get(&workspace.project_id)
            .map_err(|_| ErrorCode::RegistryIo)?
            .ok_or(ErrorCode::InvalidMessage)?;
        if workspace
            .archived_at
            .as_ref()
            .is_some_and(|value| !value.is_empty())
            || project
                .archived_at
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            || std::fs::canonicalize(&workspace.cwd).ok().as_ref() != Some(&canonical)
        {
            return Err(ErrorCode::InvalidMessage);
        }
        Ok(workspace.workspace_id)
    }

    fn resolve(&self, identifier: &str) -> Result<String, ErrorCode> {
        self.directory
            .get(identifier)
            .map(|resolved| resolved.agent.id)
            .map_err(|error| match error {
                crate::service::agent_runtime::AgentRuntimeError::NotFound(_) => {
                    ErrorCode::AgentNotFound
                }
                crate::service::agent_runtime::AgentRuntimeError::Ambiguous(_)
                | crate::service::agent_runtime::AgentRuntimeError::InvalidRequest => {
                    ErrorCode::InvalidMessage
                }
                crate::service::agent_runtime::AgentRuntimeError::AgentRegistry => {
                    ErrorCode::AgentIo
                }
                crate::service::agent_runtime::AgentRuntimeError::WorkspaceRegistry => {
                    ErrorCode::RegistryIo
                }
            })
    }

    fn snapshot(&self, id: &str) -> Result<Value, ErrorCode> {
        let record = self
            .registry
            .get(id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        self.project(&record)
    }

    fn project(&self, record: &PersistedAgentRuntimeRecord) -> Result<Value, ErrorCode> {
        let mut snapshot = serde_json::to_value(super::agent_runtime::snapshot(record))
            .map_err(|_| ErrorCode::AgentIo)?;
        snapshot["lastError"] = json!(record.last_error);
        if self.manager.live_snapshot(&record.id).is_some() {
            snapshot["providerUnavailable"] = json!(false);
            snapshot["persistence"] = json!(record.persistence);
            if let Some(turn) = self.manager.active_turn(&record.id) {
                snapshot["activeTurn"] =
                    json!({"turnId":turn,"startedAt":record.last_user_message_at});
            }
        }
        self.manager.control_snapshot(record, &mut snapshot);
        Ok(snapshot)
    }

    fn decorate(&self, value: &mut Value) -> Result<(), ErrorCode> {
        // Only replace snapshots at known response positions; arbitrary provider JSON is opaque.
        if let Some(snapshot) = value.get_mut("agent").filter(|value| value.is_object())
            && let Some(id) = snapshot.get("id").and_then(Value::as_str)
        {
            *snapshot = self.snapshot(id)?;
        }
        if let Some(entries) = value.get_mut("entries").and_then(Value::as_array_mut) {
            for entry in entries {
                self.decorate(entry)?;
            }
        }
        if let Some(agents) = value.get_mut("agents").and_then(Value::as_array_mut) {
            for agent in agents {
                if let Some(id) = agent.get("id").and_then(Value::as_str) {
                    *agent = self.snapshot(id)?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn only(params: &Value, allowed: &[&str]) -> Result<(), ErrorCode> {
    let object = params.as_object().ok_or(ErrorCode::InvalidMessage)?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ErrorCode::UnsupportedCapability);
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}

const fn map_manager(error: &AgentManagerError) -> ErrorCode {
    match error {
        AgentManagerError::NotFound(_) => ErrorCode::AgentNotFound,
        AgentManagerError::ProviderUnavailable(_)
        | AgentManagerError::MissingPersistence(_)
        | AgentManagerError::Busy => ErrorCode::UnsupportedCapability,
        AgentManagerError::InvalidRequest | AgentManagerError::AlreadyExists(_) => {
            ErrorCode::InvalidMessage
        }
        AgentManagerError::Session | AgentManagerError::Registry => ErrorCode::AgentIo,
    }
}

fn parse_creation(params: Value) -> Result<(CreateRequest, Value), ErrorCode> {
    only(
        &params,
        &[
            "agentId",
            "config",
            "workspaceId",
            "labels",
            "idempotencyKey",
            "subscribe",
            "initialPrompt",
        ],
    )?;
    only(
        &params["config"],
        &[
            "provider",
            "cwd",
            "title",
            "modeId",
            "model",
            "thinkingOptionId",
            "systemPrompt",
            "featureValues",
        ],
    )?;
    let mut intent = params.clone();
    if let Some(object) = intent.as_object_mut() {
        object.remove("idempotencyKey");
        object.remove("subscribe");
    }
    let request: CreateRequest = decode(params)?;
    if request
        .initial_prompt
        .as_ref()
        .is_some_and(|text| text.trim().is_empty() || text.len() > 65536)
    {
        return Err(ErrorCode::InvalidMessage);
    }
    if request.config.provider != "codex" {
        return Err(ErrorCode::UnsupportedCapability);
    }
    if !Path::new(&request.config.cwd).is_absolute()
        || !Path::new(&request.config.cwd).is_dir()
        || request.config.title.as_ref().is_some_and(|title| {
            title.trim().is_empty() || title.trim().encode_utf16().count() > 200
        })
        || request.labels.len() > 100
        || request
            .labels
            .iter()
            .any(|(key, value)| key.len() > 256 || value.len() > 4096)
    {
        return Err(ErrorCode::InvalidMessage);
    }
    Ok((request, intent))
}
