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
                && let Some(timeline) = &self.timeline
            {
                for mut child in timeline
                    .subagents(&record.id)
                    .map_err(|_| super::AgentManagerError::Registry)?
                {
                    if child.descriptor["status"] == "running" {
                        child.descriptor["status"] = json!("failed");
                        child.descriptor["updatedAt"] = json!(now_timestamp());
                        timeline
                            .store_subagent(&record.id, &child)
                            .map_err(|_| super::AgentManagerError::Registry)?;
                    }
                }
            }
            if !self.live.contains_key(&record.id)
                && self.clients.contains_key(&record.provider)
                && record
                    .persistence
                    .as_ref()
                    .is_some_and(|handle| handle.provider == record.provider)
                && record.attention_reason == Some(AgentAttentionReason::Permission)
                && record.persistence.as_ref().is_none_or(|handle| {
                    self.clients
                        .get(&record.provider)
                        .is_none_or(|client| client.persisted_permissions(handle).is_empty())
                })
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
        snapshot["pendingPermissions"] = json!(self.live.get(&record.id).map_or_else(
            || {
                record
                    .persistence
                    .as_ref()
                    .and_then(|handle| {
                        self.clients
                            .get(&record.provider)
                            .map(|client| client.persisted_permissions(handle))
                    })
                    .unwrap_or_default()
            },
            |agent| agent.session.pending_permissions()
        ));
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
        self.resume(id).await.map_err(|_| ErrorCode::AgentIo)?;
        let follow_up = self
            .live
            .get(id)
            .ok_or(ErrorCode::InvalidMessage)?
            .session
            .prepare_permission_response(request, response)
            .map_err(|_| ErrorCode::InvalidMessage)?;
        if let Some(prompt) = follow_up {
            let patch = self
                .live
                .get(id)
                .ok_or(ErrorCode::InvalidMessage)?
                .session
                .permission_config_patch(request, response)
                .map_err(|_| ErrorCode::InvalidMessage)?;
            if let Some(patch) = patch {
                self.configure(id, &patch)
                    .await
                    .map_err(|_| ErrorCode::AgentIo)?;
            }
            self.deliver_answer(id, &prompt).await?;
        }
        let agent = self.live.get_mut(id).ok_or(ErrorCode::InvalidMessage)?;
        if let Err(error) = agent.session.respond_permission(request, response).await {
            if error != AgentSessionError::Rejected {
                agent.pending = Some(AgentTurnEvent::Failed);
                self.poll().await.map_err(|_| ErrorCode::AgentIo)?;
                return Err(ErrorCode::AgentIo);
            }
            return Err(ErrorCode::InvalidMessage);
        }
        super::streaming::persist_handle(self.registry.as_ref(), id, agent)
            .map_err(|_| ErrorCode::AgentIo)?;
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
        let cached = self
            .timeline
            .as_ref()
            .map(|timeline| timeline.subagents(id))
            .transpose()?
            .unwrap_or_default();
        let live = self
            .live
            .get(id)
            .map(|agent| agent.session.subagents())
            .unwrap_or_default();
        let discovered = self
            .clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?
            .subagents(&record.cwd)
            .await;
        let mut children: BTreeMap<_, _> = match discovered {
            Ok(children) => children
                .into_iter()
                .map(|child| (child.id.clone(), child))
                .collect(),
            Err(_) if !live.is_empty() || !cached.is_empty() => BTreeMap::new(),
            Err(_) => return Err(ErrorCode::AgentIo),
        };
        for child in cached {
            if children.get(&child.id).is_none_or(|native| {
                child.descriptor["updatedAt"].as_str() >= native.descriptor["updatedAt"].as_str()
            }) {
                children.insert(child.id.clone(), child);
            }
        }
        for child in live {
            children.insert(child.id.clone(), child);
        }
        descendants(&handle.session_id, children.into_values().collect())
    }

    pub(crate) fn live_child(&self, parent: &str, child: &str) -> bool {
        self.live.get(parent).is_some_and(|agent| {
            agent
                .session
                .subagents()
                .iter()
                .any(|known| known.id == child)
        })
    }

    pub(crate) async fn child_history(
        &self,
        parent: &str,
        child: &NativeSubagent,
    ) -> Result<SessionHistory, ErrorCode> {
        let record = self.control_record(parent)?;
        let handle = child
            .persistence
            .clone()
            .unwrap_or_else(|| AgentPersistenceHandle {
                provider: record.provider.clone(),
                session_id: child.id.clone(),
                native_handle: None,
                metadata: None,
            });
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
                    metadata: Some({
                        let mut metadata = history.resume_metadata.clone();
                        metadata.insert(REPLACEMENT_MARKER.to_owned(), json!(true));
                        metadata
                    }),
                });
                next
            })
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        self.finish_replacement(id, &history)?;
        self.loaded_timelines.insert(id.to_owned());
        Ok(())
    }

    pub(crate) async fn rewind_mode(
        &mut self,
        id: &str,
        message: &str,
        mode: &str,
    ) -> Result<(), ErrorCode> {
        if mode == "conversation" {
            return self.rewind(id, message).await;
        }
        if !matches!(mode, "files" | "both") {
            return Err(ErrorCode::InvalidMessage);
        }
        let record = self.control_record(id)?;
        if record.archived_at.is_some() {
            return Err(ErrorCode::CatalogBusy);
        }
        let config = record.config.clone().unwrap_or_default();
        let client = self
            .clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let flag = if mode == "both" {
            "supportsRewindBoth"
        } else {
            "supportsRewindFiles"
        };
        if client.settings(&config)["capabilities"][flag] != true {
            return Err(ErrorCode::UnsupportedCapability);
        }
        self.stop_native(id).await?;
        let handle = record
            .persistence
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let spec = AgentSessionSpec {
            provider: record.provider.clone(),
            cwd: record.cwd.clone(),
            config,
        };
        self.clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?
            .rewind_files(handle, &spec, message)
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        if mode == "both" {
            self.rewind(id, message).await?;
        } else {
            self.resume(id).await.map_err(|_| ErrorCode::AgentIo)?;
        }
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

pub(super) fn resolve_permission(
    registry: &dyn crate::ports::agent_runtime::AgentRuntimeRegistry,
    timeline: Option<&crate::storage::timeline::Timeline>,
    agent: &super::LiveAgent,
    request: &str,
) -> Result<(), super::AgentManagerError> {
    let id = &agent.record.id;
    if agent.session.pending_permissions().is_empty() {
        registry
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
            .map_err(super::map_registry)?;
    }
    if let Some(timeline) = timeline {
        timeline.events().publish(
            id,
            "agent_stream",
            &json!({"agentId":id,"event":{
            "type":"permission_resolved","provider":agent.record.provider,"requestId":request,
            "resolution":{"behavior":"deny","message":"Resolved by native provider"}}}),
        );
    }
    Ok(())
}

pub(super) fn publish_subagent(
    timeline: Option<&crate::storage::timeline::Timeline>,
    agent: &super::LiveAgent,
    event: &crate::ports::controls::SubagentEvent,
) -> Result<(), ErrorCode> {
    use crate::ports::controls::SubagentEvent;
    let Some(timeline) = timeline else {
        return Ok(());
    };
    let root = agent.session.persistence().ok_or(ErrorCode::AgentIo)?;
    let descendants = descendants(&root.session_id, agent.session.subagents())?;
    let id = match event {
        SubagentEvent::Upsert(child) => &child.id,
        SubagentEvent::Progress { id, .. } | SubagentEvent::Timeline { id, .. } => id,
    };
    if !descendants.iter().any(|child| &child.id == id) {
        return Err(ErrorCode::InvalidMessage);
    }
    let parent = &agent.record.id;
    let scope = format!("subagent:{parent}:{id}");
    match event {
        SubagentEvent::Upsert(child) => {
            timeline.store_subagent(parent, child)?;
            let mut descriptor = child.descriptor.clone();
            descriptor["parentAgentId"] = json!(parent);
            descriptor["parentSubagentId"] =
                json!((child.parent_id != root.session_id).then_some(&child.parent_id));
            timeline.events().publish(
                parent,
                "agent.provider_subagents.update",
                &json!({"kind":"upsert","subagent":descriptor}),
            );
            Ok(())
        }
        SubagentEvent::Progress {
            observation, entry, ..
        } => timeline.progress(&scope, &agent.record.provider, observation, entry),
        SubagentEvent::Timeline { entry, .. } => timeline
            .append(&scope, &agent.record.provider, std::slice::from_ref(entry))
            .map(|_| ()),
    }
}

pub(super) fn publish_children(
    timeline: Option<&crate::storage::timeline::Timeline>,
    agent: &super::LiveAgent,
) -> Result<(), super::AgentManagerError> {
    for child in agent.session.subagents() {
        publish_subagent(
            timeline,
            agent,
            &crate::ports::controls::SubagentEvent::Upsert(child),
        )
        .map_err(|_| super::AgentManagerError::Registry)?;
    }
    Ok(())
}
