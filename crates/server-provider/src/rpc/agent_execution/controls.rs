//! Native controls serialized with turns and configuration updates.

use serde_json::{Value, json};

use super::{ErrorCode, ExecutionState, decode, only};
use crate::ports::agent_session::AgentSessionSpec;
use crate::protocol::agent_config::ConfigPatch;
use crate::protocol::controls::{
    CommandsRequest, PermissionRequest, RewindRequest, SubagentRequest,
};
use crate::protocol::timeline::{FetchRequest, Projection};
use crate::service::agent_manager::native_sessions::canonical;

impl ExecutionState {
    pub(super) async fn controls(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, ErrorCode> {
        match method {
            "provider.diagnostic.request" => {
                only(&params, &["provider"])?;
                let provider = params["provider"]
                    .as_str()
                    .ok_or(ErrorCode::InvalidMessage)?;
                self.manager.diagnostic(provider).await.map_err(Into::into)
            }
            "provider.usage.list.request" => {
                only(&params, &[])?;
                self.manager.usage().await.map_err(Into::into)
            }
            "agent.commands.list.request" => self.commands(params).await,
            "agent.rewind.request" => {
                let request: RewindRequest = decode(params)?;
                let id = self.resolve(&request.agent_id)?;
                self.active_workspace(&id)?;
                let result = if request.mode != "conversation" {
                    Err(server_model::ErrorCode::UnsupportedCapability)
                } else if request.message_id.is_empty() || request.message_id.len() > 512 {
                    Err(server_model::ErrorCode::InvalidMessage)
                } else {
                    self.manager.rewind(&id, &request.message_id).await
                };
                Ok(
                    json!({"agentId":id,"ok":result.is_ok(),"error":result.err().map(|error|format!("{error:?}"))}),
                )
            }
            "agent.permission.resolve.request" => {
                let request: PermissionRequest = decode(params)?;
                let id = self.resolve(&request.agent_id)?;
                self.active_workspace(&id)?;
                self.manager
                    .permission(&id, &request.request_id, &request.response)
                    .await?;
                Ok(
                    json!({"agentId":id,"requestId":request.request_id,"resolution":request.response}),
                )
            }
            "agent.provider_subagents.list.request"
            | "agent.provider_subagents.timeline.get.request" => {
                self.subagents(method, params).await
            }
            _ => Err(ErrorCode::MethodNotFound),
        }
    }

    pub(super) async fn configure(&self, method: &str, params: &Value) -> Result<Value, ErrorCode> {
        let field = match method {
            "agent.model.set.request" => "modelId",
            "agent.thinking.set.request" => "thinkingOptionId",
            "agent.mode.set.request" => "modeId",
            "agent.feature.set.request" => "value",
            _ => "config",
        };
        let allowed = if field == "value" {
            vec!["agentId", "featureId", "value"]
        } else {
            vec!["agentId", field]
        };
        only(params, &allowed)?;
        let id = self.resolve(
            params["agentId"]
                .as_str()
                .ok_or(ErrorCode::InvalidMessage)?,
        )?;
        self.active_workspace(&id)?;
        let value = params.get(field).ok_or(ErrorCode::InvalidMessage)?;
        let patch = match field {
            "config" => {
                only(
                    value,
                    &["modelId", "thinkingOptionId", "modeId", "featureValues"],
                )?;
                value.clone()
            }
            "value" => {
                let feature = params["featureId"]
                    .as_str()
                    .ok_or(ErrorCode::InvalidMessage)?;
                json!({"featureValues":{(feature):value}})
            }
            _ => json!({(field):value}),
        };
        if patch
            .get("featureValues")
            .is_some_and(|value| !value.is_object())
        {
            return Err(ErrorCode::InvalidMessage);
        }
        let patch: ConfigPatch = decode(patch)?;
        let result = self.manager.configure(&id, &patch).await;
        let notice = (result.is_ok() && self.manager.active_turn(&id).is_some())
            .then(|| json!({"type":"warning","message":"Configuration applies next turn"}));
        Ok(
            json!({"agentId":id,"accepted":result.is_ok(),"error":result.err().map(|error|error.to_string()),"notice":notice}),
        )
    }

    async fn commands(&self, params: Value) -> Result<Value, ErrorCode> {
        if let Some(config) = params.get("draftConfig").filter(|value| !value.is_null()) {
            only(
                config,
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
        }
        let request: CommandsRequest = decode(params)?;
        let (id, spec) = match self.resolve(&request.agent_id) {
            Ok(id) => {
                let record = self
                    .registry
                    .get(&id)
                    .map_err(|_| ErrorCode::AgentIo)?
                    .ok_or(ErrorCode::AgentNotFound)?;
                (
                    id,
                    AgentSessionSpec {
                        provider: record.provider,
                        cwd: record.cwd,
                        config: record.config.unwrap_or_default(),
                    },
                )
            }
            Err(ErrorCode::AgentNotFound) => {
                let draft = request.draft_config.ok_or(ErrorCode::AgentNotFound)?;
                (
                    request.agent_id,
                    AgentSessionSpec {
                        provider: draft.provider,
                        cwd: canonical(&draft.cwd)?,
                        config: draft.stored,
                    },
                )
            }
            Err(error) => return Err(error),
        };
        let commands = self.manager.commands(&spec).await?;
        crate::rpc::timeline::bounded(json!({"agentId":id,"commands":commands,"error":null}))
            .map_err(Into::into)
    }

    async fn subagents(&self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        let list = method == "agent.provider_subagents.list.request";
        if list {
            only(&params, &["parentAgentId"])?;
        }
        let request: SubagentRequest = decode(params)?;
        let id = self.resolve(&request.parent_agent_id)?;
        let record = self
            .registry
            .get(&id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        let children = self.manager.subagents(&id).await?;
        if list {
            let root = record
                .persistence
                .as_ref()
                .ok_or(ErrorCode::UnsupportedCapability)?;
            let subagents: Vec<_> = children
                .into_iter()
                .map(|child| {
                    let mut value = child.descriptor;
                    value["parentAgentId"] = json!(id);
                    value["parentSubagentId"] =
                        json!((child.parent_id != root.session_id).then_some(child.parent_id));
                    value
                })
                .collect();
            return crate::rpc::timeline::bounded(
                json!({"parentAgentId":id,"subagents":subagents,"error":null}),
            )
            .map_err(Into::into);
        }
        let child_id = request.subagent_id.ok_or(ErrorCode::InvalidMessage)?;
        let child = children
            .iter()
            .find(|child| child.id == child_id)
            .ok_or(ErrorCode::AgentNotFound)?;
        let history = self.manager.child_history(&id, child).await?;
        let timeline = self
            .manager
            .timeline()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let scope = format!("subagent:{id}:{child_id}");
        timeline.reconcile(&scope, &record.provider, &history.entries)?;
        let (epoch, rows) = timeline.read(&scope)?;
        let fetch = FetchRequest {
            agent_id: id.clone(),
            direction: request.direction,
            cursor: request.cursor,
            limit: request.limit,
            projection: Projection::Projected,
            merge_window: None,
        };
        let mut value = crate::rpc::timeline::fetch(&fetch, &epoch, &rows, &Value::Null)?;
        let object = value.as_object_mut().ok_or(ErrorCode::AgentIo)?;
        object.remove("agent");
        object.remove("agentId");
        let mut entries = object.remove("entries").ok_or(ErrorCode::AgentIo)?;
        if let Some(entries) = entries.as_array_mut() {
            for entry in entries {
                entry["seq"] = entry["seqStart"].clone();
            }
        }
        object.insert("rows".to_owned(), entries);
        object.insert("parentAgentId".to_owned(), json!(id));
        object.insert("subagentId".to_owned(), json!(child_id));
        object.insert("provider".to_owned(), json!(record.provider));
        crate::rpc::timeline::bounded(value).map_err(Into::into)
    }

    fn active_workspace(&self, id: &str) -> Result<(), ErrorCode> {
        let record = self
            .registry
            .get(id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        self.workspace(record.workspace_id.as_deref(), &record.cwd)?;
        Ok(())
    }
}
