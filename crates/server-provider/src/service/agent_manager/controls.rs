use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{Value, json};
use server_domain::agent_runtime::{AgentPersistenceHandle, PersistedAgentRuntimeRecord};
use server_model::ErrorCode;

use super::{AgentManager, now_timestamp};
use crate::ports::agent_session::{AgentSessionError, AgentSessionSpec, AgentTurnEvent};
use crate::ports::controls::NativeSubagent;
use crate::ports::native_history::SessionHistory;

pub(super) const REPLACEMENT_MARKER: &str = "aitTimelineReplacement";

impl AgentManager {
    pub(crate) fn recover_permissions(&self) -> Result<(), super::AgentManagerError> {
        use server_domain::agent_runtime::AgentAttentionReason;
        for record in self.registry.list().map_err(super::map_registry)? {
            if !self.live.contains_key(&record.id)
                && self.clients.contains_key(&record.provider)
                && record
                    .persistence
                    .as_ref()
                    .is_some_and(|handle| handle.provider == record.provider)
                && record.attention_reason == Some(AgentAttentionReason::Permission)
            {
                self.registry
                    .update(&record.id, &|current| {
                        let mut next = current.clone();
                        if next.attention_reason == Some(AgentAttentionReason::Permission) {
                            next.requires_attention = false;
                            next.attention_reason = None;
                            next.attention_timestamp = None;
                        }
                        next
                    })
                    .map_err(super::map_registry)?;
            }
        }
        Ok(())
    }

    pub(crate) fn control_snapshot(
        &self,
        record: &PersistedAgentRuntimeRecord,
        snapshot: &mut Value,
    ) {
        if let Some(client) = self.clients.get(&record.provider) {
            let settings = client.settings(&record.config.clone().unwrap_or_default());
            snapshot["availableModes"] = settings["availableModes"].clone();
            snapshot["features"] = settings["features"].clone();
            if let (Some(target), Some(flags)) = (
                snapshot["capabilities"].as_object_mut(),
                settings["capabilities"].as_object(),
            ) {
                target.extend(flags.clone());
            }
        }
        snapshot["pendingPermissions"] = json!(
            self.live
                .get(&record.id)
                .map(|agent| agent.session.pending_permissions())
                .unwrap_or_default()
        );
    }

    pub(crate) async fn diagnostic(&self, provider: &str) -> Result<Value, ErrorCode> {
        let client = self
            .clients
            .get(provider)
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let diagnostic = client
            .diagnostic()
            .await
            .unwrap_or_else(|_| "Provider inspection unavailable".to_owned());
        Ok(json!({"provider":provider,"diagnostic":diagnostic}))
    }

    pub(crate) async fn usage(&self) -> Result<Value, ErrorCode> {
        let mut providers = Vec::new();
        for (id, client) in &self.clients {
            providers.push(client.usage().await.unwrap_or_else(|_|json!({"providerId":id,"displayName":id,
                "status":"unavailable","planLabel":null,"windows":[],"error":"Provider usage is unavailable"})));
        }
        crate::rpc::timeline::bounded(json!({"fetchedAt":now_timestamp(),"providers":providers}))
    }

    pub(crate) async fn commands(&self, spec: &AgentSessionSpec) -> Result<Vec<Value>, ErrorCode> {
        self.clients
            .get(&spec.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?
            .commands(spec)
            .await
            .map_err(|_| ErrorCode::AgentIo)
    }

    pub(crate) async fn permission(
        &mut self,
        id: &str,
        request: &str,
        response: &Value,
    ) -> Result<(), ErrorCode> {
        let agent = self.live.get_mut(id).ok_or(ErrorCode::InvalidMessage)?;
        if let Err(error) = agent.session.respond_permission(request, response).await {
            if error != AgentSessionError::Rejected {
                agent.pending = Some(AgentTurnEvent::Failed);
                self.poll().await.map_err(|_| ErrorCode::AgentIo)?;
                return Err(ErrorCode::AgentIo);
            }
            return Err(ErrorCode::InvalidMessage);
        }
        let empty = agent.session.pending_permissions().is_empty();
        if empty {
            self.registry
                .update(id, &|current| {
                    let mut next = current.clone();
                    if next.attention_reason
                        == Some(server_domain::agent_runtime::AgentAttentionReason::Permission)
                    {
                        next.requires_attention = false;
                        next.attention_reason = None;
                        next.attention_timestamp = None;
                    }
                    next
                })
                .map_err(|_| ErrorCode::AgentIo)?;
        }
        if let Some(timeline) = &self.timeline {
            timeline.events().publish(id,"agent_stream",&json!({"agentId":id,"event":{"type":"permission_resolved","provider":agent.record.provider,"requestId":request,"resolution":response}}));
        }
        Ok(())
    }

    pub(crate) async fn subagents(&self, id: &str) -> Result<Vec<NativeSubagent>, ErrorCode> {
        let record = self.control_record(id)?;
        let handle = record
            .persistence
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let discovered = self
            .clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?
            .subagents(&record.cwd)
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        descendants(&handle.session_id, discovered)
    }

    pub(crate) async fn child_history(
        &self,
        parent: &str,
        child: &NativeSubagent,
    ) -> Result<SessionHistory, ErrorCode> {
        let record = self.control_record(parent)?;
        let handle = AgentPersistenceHandle {
            provider: record.provider.clone(),
            session_id: child.id.clone(),
            native_handle: None,
            metadata: None,
        };
        let history = self
            .clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?
            .inspect_session(&handle, &child.cwd)
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        if history.parent_id.as_deref() != Some(&child.parent_id)
            || history.descriptor.provider_handle_id != child.id
        {
            return Err(ErrorCode::InvalidMessage);
        }
        Ok(history)
    }

    pub(crate) async fn rewind(&mut self, id: &str, message: &str) -> Result<(), ErrorCode> {
        let record = self.control_record(id)?;
        if record.archived_at.is_some() {
            return Err(ErrorCode::CatalogBusy);
        }
        self.stop_native(id).await?;
        let handle = record
            .persistence
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let spec = AgentSessionSpec {
            provider: record.provider.clone(),
            cwd: record.cwd.clone(),
            config: record.config.clone().unwrap_or_default(),
        };
        let history = self
            .clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?
            .rewind(handle, &spec, message)
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        if history.descriptor.provider_id != record.provider
            || history.descriptor.provider_handle_id == handle.session_id
            || history.active
            || super::native_sessions::canonical(&history.descriptor.cwd)?
                != super::native_sessions::canonical(&record.cwd)?
        {
            return Err(ErrorCode::AgentIo);
        }
        self.loaded_timelines.remove(id);
        self.registry
            .update(id, &|current| {
                let mut next = current.clone();
                next.persistence = Some(AgentPersistenceHandle {
                    provider: record.provider.clone(),
                    session_id: history.descriptor.provider_handle_id.clone(),
                    native_handle: None,
                    metadata: Some(BTreeMap::from([(
                        REPLACEMENT_MARKER.to_owned(),
                        json!(true),
                    )])),
                });
                next
            })
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        self.finish_replacement(id, &history)?;
        self.loaded_timelines.insert(id.to_owned());
        Ok(())
    }

    pub(super) fn finish_replacement(
        &self,
        id: &str,
        history: &SessionHistory,
    ) -> Result<(), ErrorCode> {
        self.timeline
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?
            .reconcile(id, &history.descriptor.provider_id, &history.entries)?;
        self.registry
            .update(id, &|current| {
                let mut next = current.clone();
                let archived_at = next.archived_at.clone();
                super::native_sessions::apply_history(&mut next, history);
                next.archived_at = archived_at;
                if let Some(metadata) = next
                    .persistence
                    .as_mut()
                    .and_then(|handle| handle.metadata.as_mut())
                {
                    metadata.remove(REPLACEMENT_MARKER);
                }
                next.requires_attention = false;
                next.attention_reason = None;
                next.attention_timestamp = None;
                next
            })
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        Ok(())
    }

    fn control_record(&self, id: &str) -> Result<PersistedAgentRuntimeRecord, ErrorCode> {
        self.registry
            .get(id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)
    }
}

fn descendants(
    root: &str,
    children: Vec<NativeSubagent>,
) -> Result<Vec<NativeSubagent>, ErrorCode> {
    let mut parents: BTreeMap<String, Vec<NativeSubagent>> = BTreeMap::new();
    for child in children {
        parents
            .entry(child.parent_id.clone())
            .or_default()
            .push(child);
    }
    let mut queue = VecDeque::from([root.to_owned()]);
    let mut seen = BTreeSet::from([root.to_owned()]);
    let mut result = Vec::new();
    while let Some(parent) = queue.pop_front() {
        for child in parents.remove(&parent).unwrap_or_default() {
            if !seen.insert(child.id.clone()) {
                return Err(ErrorCode::AgentIo);
            }
            queue.push_back(child.id.clone());
            result.push(child);
        }
    }
    result.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(result)
}

#[cfg(test)]
mod tests;

pub(super) fn publish_permission(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    timeline: Option<&crate::storage::timeline::Timeline>,
    events: &server_metadata::service::session::SessionEvents,
    agent: &super::LiveAgent,
    request: &Value,
) -> Result<(), super::AgentManagerError> {
    let id = &agent.record.id;
    let now = now_timestamp();
    registry
        .update(id, &|current| {
            let mut next = current.clone();
            next.requires_attention = true;
            next.attention_reason =
                Some(server_domain::agent_runtime::AgentAttentionReason::Permission);
            next.attention_timestamp = Some(now.clone());
            next
        })
        .map_err(super::map_registry)?;
    if let Some(timeline) = timeline {
        timeline.events().publish(id,"agent_stream",&json!({"agentId":id,"event":{"type":"permission_requested","provider":agent.record.provider,"request":request}}));
    }
    events.publish(
        server_metadata::protocol::session::SessionEventKind::AgentAttention,
        &json!({"agentId":id,"reason":"permission","timestamp":now}),
    );
    Ok(())
}
