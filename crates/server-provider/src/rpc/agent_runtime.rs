//! Transport-independent agent runtime request handling.

use std::collections::BTreeSet;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use server_domain::agent_runtime::{
    AgentAttentionReason as DomainAttentionReason, AgentRuntimeStatus, PersistedAgentRuntimeRecord,
};
use server_metadata::protocol::workspace::{ProjectCheckoutLitePayload, ProjectPlacementPayload};

use crate::protocol::agent_lifecycle::{
    AgentActionResult, AgentArchiveResult, AgentAttentionClearRequest, AgentAttentionClearResult,
    AgentCapabilityFlags, AgentDeleteResult, AgentDirectoryEntry, AgentDirectoryFilter,
    AgentDirectoryResult, AgentGetRequest, AgentGetResult, AgentHistoryRequest, AgentIdRequest,
    AgentIdSelection, AgentItemsCloseRequest, AgentItemsCloseResult, AgentListRequest,
    AgentPageInfo, AgentPersistenceHandle, AgentRuntimeInfo, AgentSnapshotPayload, AgentSort,
    AgentSortKey, AgentStatus, AgentUpdateRequest, ClosedAgentResult, SortDirection,
};
use crate::rpc::ErrorCode;
use crate::service::agent_runtime::{
    AgentDirectoryEntry as ApplicationEntry, AgentDirectoryPage as ApplicationPage,
    AgentDirectoryQuery, AgentPlacement, AgentRuntimeDirectory, AgentRuntimeError,
    AgentSort as ApplicationSort, AgentSortKey as ApplicationSortKey, ResolvedAgent,
    SortDirection as ApplicationSortDirection, effective_thinking_option_id, resolved_updated_at,
};

/// Decode and execute a business request.
///
/// # Errors
/// Returns stable business failures for invalid or unsuccessful requests.
pub fn execute(
    directory: &mut AgentRuntimeDirectory,
    method: &str,
    params: Value,
) -> Result<Value, ErrorCode> {
    match method {
        "agent.list.request" => list(directory, decode(params)?),
        "agent.history.get.request" => history(directory, decode(params)?),
        "agent.get.request" => get(directory, &decode(params)?),
        "agent.update.request" => update(directory, decode(params)?),
        "agent.archive.request" => archive(directory, decode(params)?),
        "agent.delete.request" => delete(directory, decode(params)?),
        "agent.detach.request" => detach(directory, decode(params)?),
        "agent.attention.clear.request" => clear_attention(directory, decode(params)?),
        "agent.items.close.request" => close_items(directory, &decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn list(directory: &AgentRuntimeDirectory, request: AgentListRequest) -> Result<Value, ErrorCode> {
    if request
        .scope
        .as_deref()
        .is_some_and(|scope| scope != "active")
    {
        return Err(ErrorCode::InvalidMessage);
    }
    if request.subscribe.is_some() || request.sync.is_some() {
        return Err(ErrorCode::UnsupportedCapability);
    }
    let query = query(
        request.filter,
        request.sort,
        request.page,
        if request.scope.as_deref() == Some("active") {
            QueryScope::Active
        } else {
            QueryScope::All
        },
        None,
    )?;
    encode(page(
        &directory.list(&query).map_err(|error| map_error(&error))?,
    ))
}

fn history(
    directory: &AgentRuntimeDirectory,
    request: AgentHistoryRequest,
) -> Result<Value, ErrorCode> {
    let query = query(
        request.filter,
        request.sort,
        request.page,
        QueryScope::History,
        request.search,
    )?;
    encode(page(
        &directory
            .history(&query)
            .map_err(|error| map_error(&error))?,
    ))
}

fn get(directory: &AgentRuntimeDirectory, request: &AgentGetRequest) -> Result<Value, ErrorCode> {
    let result = match directory.get(&request.agent_id) {
        Ok(resolved) => get_result(&resolved),
        Err(error @ (AgentRuntimeError::NotFound(_) | AgentRuntimeError::Ambiguous(_))) => {
            AgentGetResult {
                agent: None,
                project: None,
                error: Some(error.to_string()),
            }
        }
        Err(error) => return Err(map_error(&error)),
    };
    encode(result)
}

fn update(
    directory: &AgentRuntimeDirectory,
    request: AgentUpdateRequest,
) -> Result<Value, ErrorCode> {
    let agent_id = request.agent_id;
    let result = match directory.update(
        &agent_id,
        request.name.as_deref(),
        request.labels.as_ref(),
        &timestamp(),
    ) {
        Ok(_) => AgentActionResult {
            agent_id,
            accepted: true,
            error: None,
        },
        Err(AgentRuntimeError::InvalidRequest | AgentRuntimeError::NotFound(_)) => {
            AgentActionResult {
                agent_id,
                accepted: false,
                error: Some("Nothing to update or Agent not found".to_owned()),
            }
        }
        Err(error) => return Err(map_error(&error)),
    };
    encode(result)
}

fn archive(directory: &AgentRuntimeDirectory, request: AgentIdRequest) -> Result<Value, ErrorCode> {
    let agent_id = request.agent_id;
    let record = directory
        .archive(&agent_id, &timestamp())
        .map_err(|error| map_error(&error))?;
    encode(AgentArchiveResult {
        agent_id,
        archived_at: record.archived_at.ok_or(ErrorCode::AgentIo)?,
    })
}

fn delete(directory: &AgentRuntimeDirectory, request: AgentIdRequest) -> Result<Value, ErrorCode> {
    directory
        .delete(&request.agent_id)
        .map_err(|error| map_error(&error))?;
    encode(AgentDeleteResult {
        agent_id: request.agent_id,
    })
}

fn detach(directory: &AgentRuntimeDirectory, request: AgentIdRequest) -> Result<Value, ErrorCode> {
    let agent_id = request.agent_id;
    let result = match directory.detach(&agent_id, &timestamp()) {
        Ok(_) => AgentActionResult {
            agent_id,
            accepted: true,
            error: None,
        },
        Err(error) => AgentActionResult {
            agent_id,
            accepted: false,
            error: Some(error.to_string()),
        },
    };
    encode(result)
}

fn clear_attention(
    directory: &AgentRuntimeDirectory,
    request: AgentAttentionClearRequest,
) -> Result<Value, ErrorCode> {
    let ids = match &request.agent_id {
        AgentIdSelection::One(agent_id) => vec![agent_id.clone()],
        AgentIdSelection::Many(agent_ids) => agent_ids.clone(),
    };
    if ids.is_empty() {
        return Err(ErrorCode::InvalidMessage);
    }
    let records = directory
        .clear_attention(&ids, &timestamp())
        .map_err(|error| map_error(&error))?;
    encode(AgentAttentionClearResult {
        agent_id: request.agent_id,
        agents: records.iter().map(snapshot).collect(),
    })
}

fn close_items(
    directory: &AgentRuntimeDirectory,
    request: &AgentItemsCloseRequest,
) -> Result<Value, ErrorCode> {
    if !request.terminal_ids.is_empty() {
        return Err(ErrorCode::UnsupportedCapability);
    }
    let archived_at = timestamp();
    let agents = request
        .agent_ids
        .iter()
        .filter_map(|agent_id| {
            directory
                .archive(agent_id, &archived_at)
                .ok()
                .and_then(|record| {
                    record.archived_at.map(|archived_at| ClosedAgentResult {
                        agent_id: agent_id.clone(),
                        archived_at,
                    })
                })
        })
        .collect();
    encode(AgentItemsCloseResult {
        agents,
        terminals: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy)]
enum QueryScope {
    Active,
    All,
    History,
}

fn query(
    filter: Option<AgentDirectoryFilter>,
    sort: Option<Vec<AgentSort>>,
    page: Option<crate::protocol::agent_lifecycle::AgentPageRequest>,
    scope: QueryScope,
    search: Option<String>,
) -> Result<AgentDirectoryQuery, ErrorCode> {
    let filter = filter.unwrap_or_default();
    let (offset, limit) = match page {
        Some(page) if (1..=200).contains(&page.limit) => (
            page.cursor
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| ErrorCode::InvalidMessage)?,
            page.limit,
        ),
        Some(_) => return Err(ErrorCode::InvalidMessage),
        None => (None, 200),
    };
    Ok(AgentDirectoryQuery {
        active_scope: matches!(scope, QueryScope::Active),
        include_archived: filter
            .include_archived
            .unwrap_or(matches!(scope, QueryScope::History)),
        labels: filter.labels.unwrap_or_default(),
        project_keys: filter.project_keys.and_then(|keys| {
            let keys = keys
                .into_iter()
                .filter(|key| !key.trim().is_empty())
                .collect::<BTreeSet<_>>();
            (!keys.is_empty()).then_some(keys)
        }),
        statuses: filter.statuses.and_then(|statuses| {
            let statuses = statuses
                .into_iter()
                .map(domain_status)
                .collect::<BTreeSet<_>>();
            (!statuses.is_empty()).then_some(statuses)
        }),
        requires_attention: filter.requires_attention,
        thinking_option_id: filter.thinking_option_id,
        search,
        sort: sort
            .unwrap_or_default()
            .into_iter()
            .map(application_sort)
            .collect(),
        offset: offset.unwrap_or_default(),
        limit,
    })
}

fn page(page: &ApplicationPage) -> AgentDirectoryResult {
    AgentDirectoryResult {
        entries: page.entries.iter().map(entry).collect(),
        page_info: AgentPageInfo {
            next_cursor: page.next_offset.map(|offset| offset.to_string()),
            prev_cursor: page.previous_offset.map(|offset| offset.to_string()),
            has_more: page.has_more,
        },
    }
}

fn entry(entry: &ApplicationEntry) -> AgentDirectoryEntry {
    AgentDirectoryEntry {
        agent: snapshot(&entry.agent),
        project: project(&entry.placement),
    }
}

fn get_result(resolved: &ResolvedAgent) -> AgentGetResult {
    AgentGetResult {
        agent: Some(snapshot(&resolved.agent)),
        project: resolved.placement.as_ref().map(project),
        error: None,
    }
}

pub(crate) fn snapshot(record: &PersistedAgentRuntimeRecord) -> AgentSnapshotPayload {
    let configured_thinking = record
        .config
        .as_ref()
        .and_then(|config| config.thinking_option_id.clone());
    let runtime_info = record.runtime_info.as_ref().map(|info| AgentRuntimeInfo {
        provider: info.provider.clone(),
        session_id: info.session_id.clone(),
        model: info.model.clone(),
        thinking_option_id: info.thinking_option_id.clone(),
        mode_id: info.mode_id.clone(),
        extra: info.extra.clone(),
    });
    let effective_thinking_option_id = effective_thinking_option_id(record).map(str::to_owned);
    AgentSnapshotPayload {
        id: record.id.clone(),
        provider: record.provider.clone(),
        cwd: record.cwd.clone(),
        workspace_id: record
            .workspace_id
            .clone()
            .filter(|workspace_id| !workspace_id.is_empty()),
        model: record
            .config
            .as_ref()
            .and_then(|config| config.model.clone()),
        features: Vec::new(),
        thinking_option_id: configured_thinking,
        effective_thinking_option_id,
        created_at: wire_timestamp(&record.created_at),
        updated_at: wire_timestamp(resolved_updated_at(record)),
        last_user_message_at: record.last_user_message_at.as_deref().map(wire_timestamp),
        status: protocol_status(record.last_status),
        active_turn: None,
        capabilities: stored_capabilities(),
        current_mode_id: record.last_mode_id.clone(),
        available_modes: Vec::new(),
        pending_permissions: Vec::new(),
        persistence: None::<AgentPersistenceHandle>,
        runtime_info,
        last_error: None,
        title: record.title.clone(),
        labels: record.labels.clone(),
        requires_attention: record.requires_attention,
        attention_reason: record.attention_reason.map(protocol_attention),
        attention_timestamp: record.attention_timestamp.clone(),
        archived_at: record.archived_at.clone(),
        provider_unavailable: true,
    }
}

fn project(placement: &AgentPlacement) -> ProjectPlacementPayload {
    ProjectPlacementPayload {
        project_key: placement.project_key.clone(),
        project_name: placement.project_name.clone(),
        workspace_name: Some(Some(placement.workspace_name.clone())),
        checkout: ProjectCheckoutLitePayload {
            cwd: placement.cwd.clone(),
            is_git: placement.is_git,
            current_branch: placement.current_branch.clone(),
            remote_url: None,
            worktree_root: placement.worktree_root.clone(),
            is_paseo_owned_worktree: placement.is_paseo_owned_worktree,
            main_repo_root: placement.main_repo_root.clone(),
        },
    }
}

fn stored_capabilities() -> AgentCapabilityFlags {
    [
        ("supportsStreaming", false),
        ("supportsSessionPersistence", true),
        ("supportsDynamicModes", false),
        ("supportsMcpServers", false),
        ("supportsReasoningStream", false),
        ("supportsToolInvocations", true),
        ("supportsRewindConversation", false),
        ("supportsRewindFiles", false),
        ("supportsRewindBoth", false),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

const fn domain_status(status: AgentStatus) -> AgentRuntimeStatus {
    match status {
        AgentStatus::Initializing => AgentRuntimeStatus::Initializing,
        AgentStatus::Idle => AgentRuntimeStatus::Idle,
        AgentStatus::Running => AgentRuntimeStatus::Running,
        AgentStatus::Error => AgentRuntimeStatus::Error,
        AgentStatus::Closed => AgentRuntimeStatus::Closed,
    }
}

const fn protocol_status(status: AgentRuntimeStatus) -> AgentStatus {
    match status {
        AgentRuntimeStatus::Initializing => AgentStatus::Initializing,
        AgentRuntimeStatus::Idle => AgentStatus::Idle,
        AgentRuntimeStatus::Running => AgentStatus::Running,
        AgentRuntimeStatus::Error => AgentStatus::Error,
        AgentRuntimeStatus::Closed => AgentStatus::Closed,
    }
}

const fn protocol_attention(
    reason: DomainAttentionReason,
) -> crate::protocol::agent_lifecycle::AgentAttentionReason {
    match reason {
        DomainAttentionReason::Finished => {
            crate::protocol::agent_lifecycle::AgentAttentionReason::Finished
        }
        DomainAttentionReason::Error => {
            crate::protocol::agent_lifecycle::AgentAttentionReason::Error
        }
        DomainAttentionReason::Permission => {
            crate::protocol::agent_lifecycle::AgentAttentionReason::Permission
        }
    }
}

const fn application_sort(sort: AgentSort) -> ApplicationSort {
    ApplicationSort {
        key: match sort.key {
            AgentSortKey::StatusPriority => ApplicationSortKey::StatusPriority,
            AgentSortKey::CreatedAt => ApplicationSortKey::CreatedAt,
            AgentSortKey::UpdatedAt => ApplicationSortKey::UpdatedAt,
            AgentSortKey::Title => ApplicationSortKey::Title,
        },
        direction: match sort.direction {
            SortDirection::Asc => ApplicationSortDirection::Asc,
            SortDirection::Desc => ApplicationSortDirection::Desc,
        },
    }
}

const fn map_error(error: &AgentRuntimeError) -> ErrorCode {
    match error {
        AgentRuntimeError::InvalidRequest | AgentRuntimeError::Ambiguous(_) => {
            ErrorCode::InvalidMessage
        }
        AgentRuntimeError::NotFound(_) => ErrorCode::AgentNotFound,
        AgentRuntimeError::AgentRegistry => ErrorCode::AgentIo,
        AgentRuntimeError::WorkspaceRegistry => ErrorCode::RegistryIo,
    }
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn wire_timestamp(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value).map_or_else(
        |_| value.to_owned(),
        |timestamp| {
            timestamp
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        },
    )
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl serde::Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::AgentIo)
}

#[cfg(test)]
mod tests;
