use std::collections::BTreeMap;

use serde_json::{Value, json};
use server_domain::agent_runtime::{
    AgentPersistenceHandle, AgentRuntimeStatus, PersistedAgentRuntimeRecord,
};
use uuid::Uuid;

use super::{ErrorCode, ExecutionState, decode, only};
use crate::ports::native_history::SessionHistory;
use crate::protocol::native_sessions::{ForkRequest, ImportRequest};
use crate::service::agent_manager::native_sessions::canonical;

impl ExecutionState {
    pub(super) async fn native_sessions(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, ErrorCode> {
        match method {
            "provider.sessions.recent.list.request" => self
                .manager
                .recent_sessions(decode(params)?)
                .await
                .map_err(Into::into),
            "agent.import.request" => self.import(params).await,
            "agent.refresh.request" => {
                only(&params, &["agentId"])?;
                let id = self.resolve(
                    params["agentId"]
                        .as_str()
                        .ok_or(ErrorCode::InvalidMessage)?,
                )?;
                let record = self
                    .registry
                    .get(&id)
                    .map_err(|_| ErrorCode::AgentIo)?
                    .ok_or(ErrorCode::AgentNotFound)?;
                self.workspace(record.workspace_id.as_deref(), &record.cwd)?;
                self.manager.refresh_native(&id).await?;
                let (_, rows) = self
                    .manager
                    .timeline()
                    .ok_or(ErrorCode::UnsupportedCapability)?
                    .read(&id)?;
                Ok(
                    json!({"status":"agent_refreshed","agentId":id,"agent":self.snapshot(&id)?,"timelineSize":rows.len()}),
                )
            }
            "agent.fork_context.request" => {
                let mut request: ForkRequest = decode(params)?;
                request.agent_id = self.resolve(&request.agent_id)?;
                self.manager.load_timeline(&request.agent_id).await?;
                let (epoch, rows) = self
                    .manager
                    .timeline()
                    .ok_or(ErrorCode::UnsupportedCapability)?
                    .read(&request.agent_id)?;
                crate::rpc::fork_context::export(
                    &request,
                    &epoch,
                    &rows,
                    &self.snapshot(&request.agent_id)?,
                )
                .map_err(Into::into)
            }
            _ => Err(ErrorCode::MethodNotFound),
        }
    }

    async fn import(&mut self, params: Value) -> Result<Value, ErrorCode> {
        let request: ImportRequest = decode(params)?;
        let handle = import_handle(&request)?;
        let cwd = canonical(&request.cwd)?;
        let matching: Vec<_> = self
            .registry
            .list()
            .map_err(|_| ErrorCode::AgentIo)?
            .into_iter()
            .filter(|record| {
                record.persistence.as_ref().is_some_and(|saved| {
                    saved.provider == handle.provider
                        && (saved.session_id == handle.session_id
                            || saved.native_handle.as_ref().and_then(Value::as_str)
                                == Some(handle.session_id.as_str()))
                })
            })
            .collect();
        if matching.len() > 1
            || matching
                .first()
                .is_some_and(|record| record.archived_at.is_none())
        {
            return Err(ErrorCode::InvalidMessage);
        }
        let previous = matching.into_iter().next();
        if let Some(record) = &previous {
            if canonical(&record.cwd)? != cwd {
                return Err(ErrorCode::InvalidMessage);
            }
            self.manager
                .close(&record.id)
                .await
                .map_err(|_| ErrorCode::AgentIo)?;
        }
        let history = self.manager.inspect_native(&handle, &cwd).await?;
        // Validate native identity and cwd before metadata is allowed to create placement records.
        let workspace = if request.workspace_id.is_none() {
            if let Some(directory) = &self.import_directory {
                Some(
                    directory
                        .open_workspace(&cwd, &chrono::Utc::now().to_rfc3339())
                        .map_err(|_| ErrorCode::RegistryIo)?
                        .workspace_id,
                )
            } else {
                None
            }
        } else {
            request.workspace_id.clone()
        };
        let workspace = self.workspace(workspace.as_deref(), &cwd)?;
        let existing = previous.is_some();
        let mut record = previous.unwrap_or_else(|| new_record(&history, handle));
        record.cwd = cwd;
        record.workspace_id = Some(workspace);
        record.labels = request.labels;
        let record = self.manager.import_native(record, &history, existing)?;
        Ok(
            json!({"status":"agent_resumed","agentId":record.id,"agent":self.snapshot(&record.id)?,"timelineSize":history.entries.len()}),
        )
    }
}

fn import_handle(request: &ImportRequest) -> Result<AgentPersistenceHandle, ErrorCode> {
    let provider = alias(request.provider_id.as_deref(), request.provider.as_deref())?;
    let session = alias(
        request.provider_handle_id.as_deref(),
        request.session_id.as_deref(),
    )?;
    if request.labels.len() > 100
        || request
            .labels
            .iter()
            .any(|(key, value)| key.len() > 256 || value.len() > 4096)
    {
        return Err(ErrorCode::InvalidMessage);
    }
    Ok(AgentPersistenceHandle {
        provider,
        session_id: session,
        native_handle: None,
        metadata: None,
    })
}

fn alias(primary: Option<&str>, legacy: Option<&str>) -> Result<String, ErrorCode> {
    if primary.is_some() && legacy.is_some() && primary != legacy {
        return Err(ErrorCode::InvalidMessage);
    }
    primary
        .or(legacy)
        .filter(|value| {
            !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .ok_or(ErrorCode::InvalidMessage)
}

fn new_record(
    history: &SessionHistory,
    handle: AgentPersistenceHandle,
) -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: Uuid::new_v4().to_string(),
        provider: handle.provider.clone(),
        cwd: history.descriptor.cwd.clone(),
        workspace_id: None,
        created_at: history.created_at.clone(),
        updated_at: history.descriptor.last_activity_at.clone(),
        last_activity_at: None,
        last_user_message_at: None,
        title: history.descriptor.title.clone(),
        labels: BTreeMap::new(),
        last_status: AgentRuntimeStatus::Idle,
        last_mode_id: Some("read-only".to_owned()),
        config: Some(history.config.clone()),
        runtime_info: None,
        features: Vec::new(),
        persistence: Some(handle),
        last_error: None,
        requires_attention: false,
        attention_reason: None,
        attention_timestamp: None,
        internal: false,
        archived_at: None,
        owner: None,
    }
}

#[cfg(test)]
mod tests;
