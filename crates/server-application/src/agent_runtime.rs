//! Durable Paseo Agent runtime directory and metadata lifecycle use cases.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use chrono::DateTime;
use server_domain::agent_runtime::{
    AgentAttentionReason, AgentRuntimeStatus, PersistedAgentRuntimeRecord,
};
use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use server_ports::agent_runtime::{AgentRuntimeRegistry, AgentRuntimeRegistryError};
use server_ports::registry::{ProjectRegistry, RegistryError, WorkspaceRegistry};

const PARENT_AGENT_ID_LABEL: &str = "paseo.parent-agent-id";
const OPEN_AGENT_TAB_LABEL_PREFIX: &str = "paseo.open-agent-tab.";
const DEFAULT_PAGE_LIMIT: usize = 200;

/// Sortable Agent directory fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSortKey {
    /// Attention and lifecycle priority.
    StatusPriority,
    /// Creation timestamp.
    CreatedAt,
    /// Update timestamp.
    UpdatedAt,
    /// Case-insensitive title.
    Title,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

/// One ordered sort term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSort {
    /// Sort field.
    pub key: AgentSortKey,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Application-level Agent directory query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentDirectoryQuery {
    /// Require active workspace/project placement.
    pub active_scope: bool,
    /// Include archived Agent records.
    pub include_archived: bool,
    /// Exact label filters.
    pub labels: BTreeMap<String, String>,
    /// Allowed project keys.
    pub project_keys: Option<BTreeSet<String>>,
    /// Allowed lifecycle statuses.
    pub statuses: Option<BTreeSet<AgentRuntimeStatus>>,
    /// Required attention value.
    pub requires_attention: Option<bool>,
    /// Configured thinking option filter; the outer option means filter presence.
    pub thinking_option_id: Option<Option<String>>,
    /// Case-insensitive history search.
    pub search: Option<String>,
    /// Ordered sort fields.
    pub sort: Vec<AgentSort>,
    /// Zero-based page offset decoded from the wire cursor.
    pub offset: usize,
    /// Requested page size.
    pub limit: usize,
}

/// Placement facts required by the Paseo Agent directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPlacement {
    /// Whether both workspace and project remain active.
    pub active: bool,
    /// Project key.
    pub project_key: String,
    /// Current project display name.
    pub project_name: String,
    /// Current workspace display name.
    pub workspace_name: String,
    /// Selected working directory.
    pub cwd: String,
    /// Whether placement is Git-backed.
    pub is_git: bool,
    /// Stored branch identity.
    pub current_branch: Option<String>,
    /// Stored checkout root.
    pub worktree_root: Option<String>,
    /// Whether Paseo owns the linked worktree.
    pub is_paseo_owned_worktree: bool,
    /// Main repository root for a managed worktree.
    pub main_repo_root: Option<String>,
}

/// One Agent directory row before transport projection.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDirectoryEntry {
    /// Durable Agent runtime snapshot.
    pub agent: PersistedAgentRuntimeRecord,
    /// Project/workspace placement.
    pub placement: AgentPlacement,
}

/// One page of Agent directory rows.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDirectoryPage {
    /// Matching rows.
    pub entries: Vec<AgentDirectoryEntry>,
    /// Offset for the next page.
    pub next_offset: Option<usize>,
    /// Offset used for this page.
    pub previous_offset: Option<usize>,
    /// Whether another matching page exists.
    pub has_more: bool,
}

/// Agent lookup result with optional placement.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAgent {
    /// Durable Agent runtime snapshot.
    pub agent: PersistedAgentRuntimeRecord,
    /// Placement when its workspace and project still exist.
    pub placement: Option<AgentPlacement>,
}

/// Return the newer valid timestamp from durable metadata and provider activity.
#[must_use]
pub fn resolved_updated_at(record: &PersistedAgentRuntimeRecord) -> &str {
    let updated = parse_timestamp(&record.updated_at);
    let activity = record.last_activity_at.as_deref().and_then(parse_timestamp);
    if activity.is_some_and(|activity| updated.is_none_or(|updated| activity > updated)) {
        record
            .last_activity_at
            .as_deref()
            .unwrap_or(&record.updated_at)
    } else {
        &record.updated_at
    }
}

/// Agent runtime application failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentRuntimeError {
    /// Query parameters are outside the supported bounds.
    #[error("invalid Agent directory request")]
    InvalidRequest,
    /// The requested Agent is missing.
    #[error("Agent not found: {0}")]
    NotFound(String),
    /// A prefix or title resolves to several Agents.
    #[error("Agent identifier is ambiguous: {0}")]
    Ambiguous(String),
    /// Runtime record persistence failed.
    #[error("Agent runtime registry failed")]
    AgentRegistry,
    /// Project/workspace placement persistence failed.
    #[error("workspace registry failed")]
    WorkspaceRegistry,
}

/// Durable Agent runtime directory independent of provider execution.
#[derive(Debug)]
pub struct AgentRuntimeDirectory {
    agents: Box<dyn AgentRuntimeRegistry>,
    workspaces: Box<dyn WorkspaceRegistry>,
    projects: Box<dyn ProjectRegistry>,
}

impl AgentRuntimeDirectory {
    /// Compose the runtime snapshot and placement registries.
    #[must_use]
    pub fn new(
        agents: Box<dyn AgentRuntimeRegistry>,
        workspaces: Box<dyn WorkspaceRegistry>,
        projects: Box<dyn ProjectRegistry>,
    ) -> Self {
        Self {
            agents,
            workspaces,
            projects,
        }
    }

    /// List active Agent directory rows.
    ///
    /// # Errors
    /// Returns invalid-query or registry failures.
    pub fn list(
        &self,
        query: &AgentDirectoryQuery,
    ) -> Result<AgentDirectoryPage, AgentRuntimeError> {
        self.query(query)
    }

    /// List Agent history, including archived records when requested by the caller.
    ///
    /// # Errors
    /// Returns invalid-query or registry failures.
    pub fn history(
        &self,
        query: &AgentDirectoryQuery,
    ) -> Result<AgentDirectoryPage, AgentRuntimeError> {
        self.query(query)
    }

    /// Resolve a full ID, unique ID prefix, or exact full title.
    ///
    /// # Errors
    /// Returns an explicit missing/ambiguous result or registry failure.
    pub fn get(&self, identifier: &str) -> Result<ResolvedAgent, AgentRuntimeError> {
        let identifier = identifier.trim();
        if identifier.is_empty() {
            return Err(AgentRuntimeError::NotFound(String::new()));
        }
        let records = self.public_records()?;
        let record = if let Some(record) = records.iter().find(|record| record.id == identifier) {
            record.clone()
        } else {
            let prefix_matches = records
                .iter()
                .filter(|record| record.id.starts_with(identifier))
                .collect::<Vec<_>>();
            match prefix_matches.as_slice() {
                [record] => (*record).clone(),
                [] => Self::resolve_title(&records, identifier)?,
                _ => return Err(AgentRuntimeError::Ambiguous(identifier.to_owned())),
            }
        };
        let placement = self
            .placements()?
            .remove(&record.workspace_id.clone().unwrap_or_default());
        Ok(ResolvedAgent {
            agent: record,
            placement,
        })
    }

    /// Update title and/or labels on one stored Agent.
    ///
    /// # Errors
    /// Returns missing-Agent, invalid-request, or persistence failures.
    pub fn update(
        &self,
        agent_id: &str,
        name: Option<&str>,
        labels: Option<&BTreeMap<String, String>>,
        updated_at: &str,
    ) -> Result<PersistedAgentRuntimeRecord, AgentRuntimeError> {
        let title = name.map(str::trim).filter(|title| !title.is_empty());
        let labels = labels.filter(|labels| !labels.is_empty());
        if title.is_none() && labels.is_none() {
            return Err(AgentRuntimeError::InvalidRequest);
        }
        self.agents
            .update(agent_id, &|current| {
                let mut next = current.clone();
                if let Some(title) = title {
                    next.title = Some(title.to_owned());
                }
                if let Some(labels) = labels {
                    next.labels.extend(labels.clone());
                }
                updated_at.clone_into(&mut next.updated_at);
                next
            })
            .map_err(map_agent_registry)?
            .ok_or_else(|| AgentRuntimeError::NotFound(agent_id.to_owned()))
    }

    /// Archive an Agent snapshot and eligible delegated children.
    ///
    /// # Errors
    /// Returns missing-Agent or persistence failures.
    pub fn archive(
        &self,
        agent_id: &str,
        archived_at: &str,
    ) -> Result<PersistedAgentRuntimeRecord, AgentRuntimeError> {
        let current = self
            .agents
            .get(agent_id)
            .map_err(map_agent_registry)?
            .ok_or_else(|| AgentRuntimeError::NotFound(agent_id.to_owned()))?;
        if current.archived_at.is_some() {
            return Ok(current);
        }
        let archived = self.archive_one(agent_id, archived_at)?;
        self.archive_children(&archived, archived_at)?;
        Ok(archived)
    }

    /// Permanently remove one Agent snapshot.
    ///
    /// # Errors
    /// Returns missing-Agent or persistence failures.
    pub fn delete(&self, agent_id: &str) -> Result<(), AgentRuntimeError> {
        if !self.agents.remove(agent_id).map_err(map_agent_registry)? {
            return Err(AgentRuntimeError::NotFound(agent_id.to_owned()));
        }
        Ok(())
    }

    /// Clear attention state for every selected Agent.
    ///
    /// # Errors
    /// Returns missing-Agent or persistence failures. Earlier updates may already be durable.
    pub fn clear_attention(
        &self,
        agent_ids: &[String],
        updated_at: &str,
    ) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeError> {
        agent_ids
            .iter()
            .map(|agent_id| {
                self.agents
                    .update(agent_id, &|current| {
                        let mut next = current.clone();
                        next.requires_attention = false;
                        next.attention_reason = None;
                        next.attention_timestamp = None;
                        updated_at.clone_into(&mut next.updated_at);
                        next
                    })
                    .map_err(map_agent_registry)?
                    .ok_or_else(|| AgentRuntimeError::NotFound(agent_id.clone()))
            })
            .collect()
    }

    /// Remove delegation and connection-owned open-tab labels.
    ///
    /// # Errors
    /// Returns missing-Agent or persistence failures.
    pub fn detach(
        &self,
        agent_id: &str,
        updated_at: &str,
    ) -> Result<PersistedAgentRuntimeRecord, AgentRuntimeError> {
        let current = self
            .agents
            .get(agent_id)
            .map_err(map_agent_registry)?
            .ok_or_else(|| AgentRuntimeError::NotFound(agent_id.to_owned()))?;
        let has_parent = current
            .labels
            .get(PARENT_AGENT_ID_LABEL)
            .is_some_and(|parent| !parent.trim().is_empty());
        if !has_parent {
            return Ok(current);
        }
        self.agents
            .update(agent_id, &|current| {
                let mut next = current.clone();
                next.labels.remove(PARENT_AGENT_ID_LABEL);
                next.labels
                    .retain(|label, _| !label.starts_with(OPEN_AGENT_TAB_LABEL_PREFIX));
                updated_at.clone_into(&mut next.updated_at);
                next
            })
            .map_err(map_agent_registry)?
            .ok_or_else(|| AgentRuntimeError::NotFound(agent_id.to_owned()))
    }

    fn query(&self, query: &AgentDirectoryQuery) -> Result<AgentDirectoryPage, AgentRuntimeError> {
        if !(1..=DEFAULT_PAGE_LIMIT).contains(&query.limit) {
            return Err(AgentRuntimeError::InvalidRequest);
        }
        let placements = self.placements()?;
        let mut entries = self
            .public_records()?
            .into_iter()
            .filter(|record| query.include_archived || record.archived_at.is_none())
            .filter_map(|agent| {
                let placement = placements.get(agent.workspace_id.as_deref()?)?.clone();
                Some(AgentDirectoryEntry { agent, placement })
            })
            .filter(|entry| matches_query(entry, query))
            .collect::<Vec<_>>();
        let sort = if query.sort.is_empty() {
            vec![AgentSort {
                key: AgentSortKey::UpdatedAt,
                direction: SortDirection::Desc,
            }]
        } else {
            query.sort.clone()
        };
        entries.sort_by(|left, right| compare_entries(left, right, &sort));
        if query.offset > entries.len() {
            return Err(AgentRuntimeError::InvalidRequest);
        }
        let end = query.offset.saturating_add(query.limit).min(entries.len());
        let has_more = end < entries.len();
        Ok(AgentDirectoryPage {
            entries: entries[query.offset..end].to_vec(),
            next_offset: has_more.then_some(end),
            previous_offset: (query.offset > 0).then_some(query.offset.saturating_sub(query.limit)),
            has_more,
        })
    }

    fn public_records(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeError> {
        Ok(self
            .agents
            .list()
            .map_err(map_agent_registry)?
            .into_iter()
            .filter(|record| !record.internal)
            .collect())
    }

    fn placements(&self) -> Result<BTreeMap<String, AgentPlacement>, AgentRuntimeError> {
        let projects = self
            .projects
            .list()
            .map_err(map_workspace_registry)?
            .into_iter()
            .map(|project| (project.project_id.clone(), project))
            .collect::<BTreeMap<_, _>>();
        Ok(self
            .workspaces
            .list()
            .map_err(map_workspace_registry)?
            .into_iter()
            .filter_map(|workspace| {
                let project = projects.get(&workspace.project_id)?;
                Some((
                    workspace.workspace_id.clone(),
                    placement(&workspace, project),
                ))
            })
            .collect())
    }

    fn resolve_title(
        records: &[PersistedAgentRuntimeRecord],
        title: &str,
    ) -> Result<PersistedAgentRuntimeRecord, AgentRuntimeError> {
        let matches = records
            .iter()
            .filter(|record| record.title.as_deref() == Some(title))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [record] => Ok((*record).clone()),
            [] => Err(AgentRuntimeError::NotFound(title.to_owned())),
            _ => Err(AgentRuntimeError::Ambiguous(title.to_owned())),
        }
    }

    fn archive_one(
        &self,
        agent_id: &str,
        archived_at: &str,
    ) -> Result<PersistedAgentRuntimeRecord, AgentRuntimeError> {
        self.agents
            .update(agent_id, &|current| {
                if current.archived_at.is_some() {
                    return current.clone();
                }
                let mut next = current.clone();
                next.archived_at = Some(archived_at.to_owned());
                if matches!(
                    next.last_status,
                    AgentRuntimeStatus::Running | AgentRuntimeStatus::Initializing
                ) {
                    next.last_status = AgentRuntimeStatus::Idle;
                }
                next.requires_attention = false;
                next.attention_reason = None;
                next.attention_timestamp = None;
                next
            })
            .map_err(map_agent_registry)?
            .ok_or_else(|| AgentRuntimeError::NotFound(agent_id.to_owned()))
    }

    fn archive_children(
        &self,
        parent: &PersistedAgentRuntimeRecord,
        archived_at: &str,
    ) -> Result<(), AgentRuntimeError> {
        let children = self
            .public_records()?
            .into_iter()
            .filter(|record| record.archived_at.is_none())
            .filter(|record| {
                record.labels.get(PARENT_AGENT_ID_LABEL).map(String::as_str)
                    == Some(parent.id.as_str())
            })
            .collect::<Vec<_>>();
        for child in children {
            let has_open_tab = child.labels.iter().any(|(label, value)| {
                label.starts_with(OPEN_AGENT_TAB_LABEL_PREFIX) && value == "true"
            });
            let cross_workspace = parent.workspace_id.is_some()
                && child.workspace_id.is_some()
                && parent.workspace_id != child.workspace_id;
            if has_open_tab || cross_workspace {
                self.detach(&child.id, archived_at)?;
            } else {
                let archived = self.archive_one(&child.id, archived_at)?;
                self.archive_children(&archived, archived_at)?;
            }
        }
        Ok(())
    }
}

fn placement(
    workspace: &PersistedWorkspaceRecord,
    project: &PersistedProjectRecord,
) -> AgentPlacement {
    let is_git = project.kind == PersistedProjectKind::Git
        || workspace.kind != PersistedWorkspaceKind::Directory;
    AgentPlacement {
        active: workspace.archived_at.as_ref().is_none_or(String::is_empty)
            && project.archived_at.as_ref().is_none_or(String::is_empty),
        project_key: project
            .project_key
            .clone()
            .unwrap_or_else(|| project.project_id.clone()),
        project_name: project.display_name().to_owned(),
        workspace_name: workspace.display_name().to_owned(),
        cwd: workspace.cwd.clone(),
        is_git,
        current_branch: workspace.branch.clone(),
        worktree_root: is_git.then(|| {
            workspace
                .worktree_root
                .clone()
                .unwrap_or_else(|| workspace.cwd.clone())
        }),
        is_paseo_owned_worktree: workspace.is_paseo_owned_worktree,
        main_repo_root: workspace.main_repo_root.clone(),
    }
}

fn matches_query(entry: &AgentDirectoryEntry, query: &AgentDirectoryQuery) -> bool {
    let agent = &entry.agent;
    if query.active_scope && (agent.archived_at.is_some() || !entry.placement.active) {
        return false;
    }
    if !query
        .labels
        .iter()
        .all(|(key, value)| agent.labels.get(key) == Some(value))
    {
        return false;
    }
    if query
        .project_keys
        .as_ref()
        .is_some_and(|keys| !keys.contains(&entry.placement.project_key))
        || query
            .statuses
            .as_ref()
            .is_some_and(|statuses| !statuses.contains(&agent.last_status))
        || query
            .requires_attention
            .is_some_and(|required| required != agent.requires_attention)
    {
        return false;
    }
    let thinking = effective_thinking_option_id(agent);
    if query
        .thinking_option_id
        .as_ref()
        .is_some_and(|required| normalize_thinking_option_id(required.as_deref()) != thinking)
    {
        return false;
    }
    query.search.as_ref().is_none_or(|search| {
        let search = search.trim().to_lowercase();
        search.is_empty()
            || agent
                .title
                .as_ref()
                .is_some_and(|title| title.to_lowercase().contains(&search))
            || entry
                .placement
                .workspace_name
                .to_lowercase()
                .contains(&search)
            || entry
                .placement
                .current_branch
                .as_ref()
                .is_some_and(|branch| branch.to_lowercase().contains(&search))
            || entry
                .placement
                .project_name
                .to_lowercase()
                .contains(&search)
    })
}

fn compare_entries(
    left: &AgentDirectoryEntry,
    right: &AgentDirectoryEntry,
    sort: &[AgentSort],
) -> Ordering {
    sort.iter()
        .find_map(|term| {
            let ordering = match term.key {
                AgentSortKey::StatusPriority => {
                    status_priority(&left.agent).cmp(&status_priority(&right.agent))
                }
                AgentSortKey::CreatedAt => {
                    compare_timestamp_values(&left.agent.created_at, &right.agent.created_at)
                }
                AgentSortKey::UpdatedAt => compare_updated_at(&left.agent, &right.agent),
                AgentSortKey::Title => left
                    .agent
                    .title
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .cmp(
                        &right
                            .agent
                            .title
                            .as_deref()
                            .unwrap_or_default()
                            .to_lowercase(),
                    ),
            };
            let ordering = match term.direction {
                SortDirection::Asc => ordering,
                SortDirection::Desc => ordering.reverse(),
            };
            (ordering != Ordering::Equal).then_some(ordering)
        })
        .unwrap_or_else(|| left.agent.id.cmp(&right.agent.id))
}

fn parse_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn compare_updated_at(
    left: &PersistedAgentRuntimeRecord,
    right: &PersistedAgentRuntimeRecord,
) -> Ordering {
    let left_value = resolved_updated_at(left);
    let right_value = resolved_updated_at(right);
    compare_timestamp_values(left_value, right_value)
}

fn compare_timestamp_values(left: &str, right: &str) -> Ordering {
    match (parse_timestamp(left), parse_timestamp(right)) {
        (Some(left_timestamp), Some(right_timestamp)) => left_timestamp
            .cmp(&right_timestamp)
            .then_with(|| left.cmp(right)),
        _ => left.cmp(right),
    }
}

/// Resolve the normalized runtime thinking selection, falling back to stored configuration.
#[must_use]
pub fn effective_thinking_option_id(record: &PersistedAgentRuntimeRecord) -> Option<&str> {
    normalize_thinking_option_id(
        record
            .runtime_info
            .as_ref()
            .and_then(|runtime| runtime.thinking_option_id.as_deref())
            .or_else(|| {
                record
                    .config
                    .as_ref()
                    .and_then(|config| config.thinking_option_id.as_deref())
            }),
    )
}

fn normalize_thinking_option_id(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn status_priority(record: &PersistedAgentRuntimeRecord) -> u8 {
    if matches!(
        record.attention_reason,
        Some(AgentAttentionReason::Permission)
    ) {
        return 0;
    }
    if record.last_status == AgentRuntimeStatus::Error
        || matches!(record.attention_reason, Some(AgentAttentionReason::Error))
    {
        return 1;
    }
    match record.last_status {
        AgentRuntimeStatus::Running => 2,
        AgentRuntimeStatus::Initializing => 3,
        AgentRuntimeStatus::Idle | AgentRuntimeStatus::Error | AgentRuntimeStatus::Closed => 4,
    }
}

const fn map_agent_registry(_error: AgentRuntimeRegistryError) -> AgentRuntimeError {
    AgentRuntimeError::AgentRegistry
}

const fn map_workspace_registry(_error: RegistryError) -> AgentRuntimeError {
    AgentRuntimeError::WorkspaceRegistry
}

#[cfg(test)]
mod tests;
