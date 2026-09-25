use std::collections::BTreeSet;
use std::time::Duration;

use chrono::DateTime;
use serde_json::{Value, json};
use server_domain::agent_runtime::{
    AgentPersistenceHandle, AgentRuntimeStatus, PersistedAgentRuntimeRecord, StoredAgentRuntimeInfo,
};
use server_model::ErrorCode;

use super::{AgentManager, now_timestamp};
use crate::ports::native_history::{ListOptions, SessionHistory};
use crate::protocol::native_sessions::RecentRequest;

impl AgentManager {
    pub(crate) async fn recent_sessions(&self, request: RecentRequest) -> Result<Value, ErrorCode> {
        let limit = request.limit.unwrap_or(20);
        if !(1..=200).contains(&limit)
            || request.providers.as_ref().is_some_and(|providers| {
                providers.len() > 32 || providers.iter().any(|id| !self.clients.contains_key(id))
            })
        {
            return Err(ErrorCode::InvalidMessage);
        }
        let since = request
            .since
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .map_err(|_| ErrorCode::InvalidMessage)?;
        let query = request.query.unwrap_or_default().trim().to_lowercase();
        if query.len() > 4096 {
            return Err(ErrorCode::InvalidMessage);
        }
        let cwd = request.cwd.map(|cwd| canonical(&cwd)).transpose()?;
        let imported = self.imported_handles()?;
        let options = ListOptions {
            cwd,
            scan_limit: if query.is_empty() {
                limit.saturating_add(imported.len()).min(4096)
            } else {
                500
            },
        };
        let mut candidates = Vec::new();
        let mut errors = Vec::new();
        let mut filtered = 0;
        for (provider, client) in &self.clients {
            if request
                .providers
                .as_ref()
                .is_some_and(|ids| !ids.contains(provider))
            {
                continue;
            }
            let Ok(sessions) = client.list_sessions(&options).await else {
                errors.push(
                    json!({"provider":provider,"message":"Provider session discovery failed"}),
                );
                continue;
            };
            let mut seen = BTreeSet::new();
            for session in sessions {
                let timestamp = DateTime::parse_from_rfc3339(&session.last_activity_at)
                    .map_err(|_| ErrorCode::AgentIo)?;
                if session.provider_id != *provider
                    || !seen.insert(session.provider_handle_id.clone())
                {
                    continue;
                }
                if options
                    .cwd
                    .as_ref()
                    .is_some_and(|cwd| canonical(&session.cwd).as_ref() != Ok(cwd))
                    || since.as_ref().is_some_and(|since| timestamp < *since)
                    || session.first_prompt_preview.as_deref().is_some_and(|text| {
                        text.trim_start().starts_with(
                            "Generate metadata for a coding agent based on the user prompt.",
                        )
                    })
                    || !matches_query(&session, &query)
                {
                    continue;
                }
                if imported.contains(&(provider.clone(), session.provider_handle_id.clone())) {
                    filtered += 1;
                } else {
                    candidates.push((timestamp, session));
                }
            }
        }
        candidates.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then(left.1.provider_id.cmp(&right.1.provider_id))
                .then(left.1.provider_handle_id.cmp(&right.1.provider_handle_id))
        });
        let entries: Vec<_> = candidates
            .into_iter()
            .take(limit)
            .map(|(_, session)| session)
            .collect();
        crate::rpc::timeline::bounded(
            json!({"entries":entries,"filteredAlreadyImportedCount":filtered,"providerErrors":errors}),
        )
    }

    fn imported_handles(&self) -> Result<BTreeSet<(String, String)>, ErrorCode> {
        Ok(self
            .registry
            .list()
            .map_err(|_| ErrorCode::AgentIo)?
            .into_iter()
            .filter(|record| record.archived_at.is_none())
            .filter_map(|record| record.persistence)
            .flat_map(|handle| {
                let mut keys = vec![(handle.provider.clone(), handle.session_id)];
                if let Some(native) = handle.native_handle.as_ref().and_then(Value::as_str) {
                    keys.push((handle.provider, native.to_owned()));
                }
                keys
            })
            .collect())
    }

    pub(crate) async fn inspect_native(
        &self,
        handle: &AgentPersistenceHandle,
        cwd: &str,
    ) -> Result<SessionHistory, ErrorCode> {
        let client = self
            .clients
            .get(&handle.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let history = client
            .inspect_session(handle, cwd)
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        if history.descriptor.provider_id != handle.provider
            || history.descriptor.provider_handle_id != handle.session_id
            || canonical(&history.descriptor.cwd)? != canonical(cwd)?
        {
            return Err(ErrorCode::InvalidMessage);
        }
        if history.active {
            return Err(ErrorCode::CatalogBusy);
        }
        Ok(history)
    }

    pub(crate) fn import_native(
        &mut self,
        mut record: PersistedAgentRuntimeRecord,
        history: &SessionHistory,
        existing: bool,
    ) -> Result<PersistedAgentRuntimeRecord, ErrorCode> {
        if self.live.contains_key(&record.id) {
            return Err(ErrorCode::CatalogBusy);
        }
        let timeline = self
            .timeline
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        timeline.reconcile(&record.id, &record.provider, &history.entries)?;
        apply_history(&mut record, history);
        let committed = if existing {
            self.registry
                .update(&record.id, &|current| {
                    let mut next = current.clone();
                    next.workspace_id.clone_from(&record.workspace_id);
                    next.labels.remove("paseo.parent-agent-id");
                    next.labels.extend(record.labels.clone());
                    apply_history(&mut next, history);
                    next
                })
                .map_err(|_| ErrorCode::AgentIo)?
                .ok_or(ErrorCode::AgentNotFound)?
        } else {
            self.registry
                .upsert(&record)
                .map_err(|_| ErrorCode::AgentIo)?;
            record
        };
        self.loaded_timelines.insert(committed.id.clone());
        Ok(committed)
    }

    pub(super) async fn stop_native(&mut self, id: &str) -> Result<(), ErrorCode> {
        if self.active_turn(id).is_some() {
            self.cancel(id).await.map_err(|_| ErrorCode::AgentIo)?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while self.active_turn(id).is_some() {
                self.poll().await.map_err(|_| ErrorCode::AgentIo)?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(ErrorCode::CatalogBusy);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        self.close(id).await.map_err(|_| ErrorCode::AgentIo)?;
        Ok(())
    }

    pub(crate) async fn refresh_native(&mut self, id: &str) -> Result<(), ErrorCode> {
        self.stop_native(id).await?;
        let record = self
            .registry
            .get(id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        let handle = record
            .persistence
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let history = self.inspect_native(handle, &record.cwd).await?;
        self.timeline
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?
            .reconcile(id, &record.provider, &history.entries)?;
        self.registry
            .update(id, &|current| {
                let mut next = current.clone();
                apply_history(&mut next, &history);
                if let Some(metadata) = next
                    .persistence
                    .as_mut()
                    .and_then(|handle| handle.metadata.as_mut())
                {
                    metadata.remove(super::controls::REPLACEMENT_MARKER);
                }
                next
            })
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        self.loaded_timelines.insert(id.to_owned());
        Ok(())
    }
}

pub(super) fn apply_history(record: &mut PersistedAgentRuntimeRecord, history: &SessionHistory) {
    record.archived_at = None;
    record.last_status = AgentRuntimeStatus::Idle;
    record.last_error = None;
    record.updated_at = now_timestamp();
    record.last_activity_at = Some(history.descriptor.last_activity_at.clone());
    record.last_user_message_at = history
        .entries
        .iter()
        .rev()
        .find(|entry| entry.item["type"] == "user_message")
        .map(|entry| entry.timestamp.clone());
    record.runtime_info = Some(StoredAgentRuntimeInfo {
        provider: record.provider.clone(),
        session_id: Some(history.descriptor.provider_handle_id.clone()),
        model: history.config.model.clone(),
        thinking_option_id: history.config.thinking_option_id.clone(),
        mode_id: record
            .config
            .as_ref()
            .and_then(|config| config.mode_id.clone())
            .or_else(|| Some("read-only".to_owned())),
        extra: None,
    });
}

pub(crate) fn canonical(cwd: &str) -> Result<String, ErrorCode> {
    let path = std::path::Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err(ErrorCode::InvalidMessage);
    }
    path.canonicalize()
        .map_err(|_| ErrorCode::InvalidMessage)?
        .into_os_string()
        .into_string()
        .map_err(|_| ErrorCode::InvalidMessage)
}

fn matches_query(session: &crate::ports::native_history::SessionDescriptor, query: &str) -> bool {
    query.is_empty()
        || [
            Some(session.provider_handle_id.as_str()),
            Some(session.cwd.as_str()),
            session.title.as_deref(),
            session.first_prompt_preview.as_deref(),
            session.last_prompt_preview.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(query))
}

#[cfg(test)]
mod tests;
