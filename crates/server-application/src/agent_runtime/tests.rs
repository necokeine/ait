use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use server_ports::agent_runtime::{AgentRuntimeRegistry, AgentRuntimeRegistryError};
use server_ports::registry::{
    ActiveProjectInput, MutationListener, MutationSubscription, ProjectMutation,
    WorkspaceArchiveContext, WorkspaceMutation, WorkspaceMutationContext,
};

use super::*;

#[derive(Debug, Clone, Default)]
struct Agents(Arc<Mutex<Vec<PersistedAgentRuntimeRecord>>>);

impl AgentRuntimeRegistry for Agents {
    fn initialize(&self) -> Result<(), AgentRuntimeRegistryError> {
        Ok(())
    }

    fn list(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        Ok(self.0.lock().expect("agents").clone())
    }

    fn get(
        &self,
        agent_id: &str,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        Ok(self
            .0
            .lock()
            .expect("agents")
            .iter()
            .find(|record| record.id == agent_id)
            .cloned())
    }

    fn upsert(
        &self,
        record: &PersistedAgentRuntimeRecord,
    ) -> Result<(), AgentRuntimeRegistryError> {
        let mut records = self.0.lock().expect("agents");
        if let Some(existing) = records.iter_mut().find(|item| item.id == record.id) {
            existing.clone_from(record);
        } else {
            records.push(record.clone());
        }
        Ok(())
    }

    fn update(
        &self,
        agent_id: &str,
        update: &dyn Fn(&PersistedAgentRuntimeRecord) -> PersistedAgentRuntimeRecord,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        let mut records = self.0.lock().expect("agents");
        let Some(record) = records.iter_mut().find(|record| record.id == agent_id) else {
            return Ok(None);
        };
        *record = update(record);
        Ok(Some(record.clone()))
    }

    fn remove(&self, agent_id: &str) -> Result<bool, AgentRuntimeRegistryError> {
        let mut records = self.0.lock().expect("agents");
        let before = records.len();
        records.retain(|record| record.id != agent_id);
        Ok(records.len() != before)
    }
}

#[derive(Debug, Clone, Default)]
struct Projects(Arc<Mutex<Vec<PersistedProjectRecord>>>);

impl ProjectRegistry for Projects {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }
    fn exists_on_disk(&self) -> bool {
        true
    }
    fn list(&self) -> Result<Vec<PersistedProjectRecord>, RegistryError> {
        Ok(self.0.lock().expect("projects").clone())
    }
    fn get(&self, id: &str) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        Ok(self
            .0
            .lock()
            .expect("projects")
            .iter()
            .find(|record| record.project_id == id)
            .cloned())
    }
    fn get_or_create_active_by_root(
        &self,
        _input: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn upsert(&self, record: &PersistedProjectRecord) -> Result<(), RegistryError> {
        self.0.lock().expect("projects").push(record.clone());
        Ok(())
    }
    fn update(
        &self,
        _id: &str,
        _update: &dyn Fn(&PersistedProjectRecord) -> PersistedProjectRecord,
    ) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn archive(&self, _id: &str, _timestamp: &str) -> Result<(), RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn remove(&self, _id: &str) -> Result<(), RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn subscribe_to_mutations(
        &self,
        _listener: MutationListener<ProjectMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
}

#[derive(Debug, Clone, Default)]
struct Workspaces(Arc<Mutex<Vec<PersistedWorkspaceRecord>>>);

impl WorkspaceRegistry for Workspaces {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }
    fn exists_on_disk(&self) -> bool {
        true
    }
    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self.0.lock().expect("workspaces").clone())
    }
    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self
            .0
            .lock()
            .expect("workspaces")
            .iter()
            .find(|record| record.workspace_id == id)
            .cloned())
    }
    fn upsert(
        &self,
        record: &PersistedWorkspaceRecord,
        _context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        self.0.lock().expect("workspaces").push(record.clone());
        Ok(())
    }
    fn update(
        &self,
        _id: &str,
        _update: &dyn Fn(&PersistedWorkspaceRecord) -> PersistedWorkspaceRecord,
    ) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn archive(
        &self,
        _id: &str,
        _timestamp: &str,
        _context: &WorkspaceArchiveContext,
    ) -> Result<(), RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn remove(&self, _id: &str) -> Result<(), RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn subscribe_to_mutations(
        &self,
        _listener: MutationListener<WorkspaceMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
    fn block_all_mutations_until_restart(&self) -> Result<(), RegistryError> {
        Ok(())
    }
}

#[derive(Debug)]
struct Subscription;
impl MutationSubscription for Subscription {}

#[test]
fn list_filters_sorts_and_pages_placed_public_agents() {
    let (service, agents) = service();
    let query = AgentDirectoryQuery {
        labels: BTreeMap::from([("team".to_owned(), "server".to_owned())]),
        statuses: Some(BTreeSet::from([AgentRuntimeStatus::Idle])),
        sort: vec![AgentSort {
            key: AgentSortKey::Title,
            direction: SortDirection::Asc,
        }],
        limit: 1,
        ..AgentDirectoryQuery::default()
    };

    let first = service.list(&query).expect("list should succeed");
    assert_eq!(first.entries[0].agent.id, "agent-a");
    assert_eq!(first.entries[0].placement.project_key, "key-one");
    assert_eq!(first.next_offset, Some(1));
    let second = service
        .list(&AgentDirectoryQuery {
            offset: first.next_offset.expect("next cursor"),
            ..query
        })
        .expect("second page should succeed");
    assert_eq!(second.entries[0].agent.id, "agent-b");
    assert_eq!(second.previous_offset, Some(0));

    agents
        .upsert(&agent("internal", "wks-one", "hidden", true))
        .expect("fixture insert");
    assert_eq!(
        service.list(&default_query()).expect("list").entries.len(),
        2
    );
}

#[test]
fn history_searches_placement_and_includes_archived_by_request() {
    let (service, agents) = service();
    agents
        .update("agent-a", &|current| {
            let mut next = current.clone();
            next.last_activity_at = Some("2026-09-22T01:00:00+01:00".to_owned());
            next
        })
        .expect("fixture update");
    let mut archived = agent("archived", "wks-one", "Old", false);
    archived.archived_at = Some("2026-09-21T00:00:00.000Z".to_owned());
    archived.labels.clear();
    agents.upsert(&archived).expect("fixture insert");

    let result = service
        .history(&AgentDirectoryQuery {
            include_archived: true,
            search: Some("main".to_owned()),
            limit: 200,
            ..AgentDirectoryQuery::default()
        })
        .expect("history should succeed");
    assert_eq!(result.entries.len(), 3);
    assert_eq!(result.entries[0].agent.id, "agent-a");
    assert!(
        result
            .entries
            .iter()
            .any(|entry| entry.agent.id == "archived")
    );
}

#[test]
fn active_scope_excludes_archived_placement_and_invalid_pages() {
    let (service, _) = service();
    assert_eq!(
        service
            .list(&AgentDirectoryQuery {
                active_scope: true,
                limit: 200,
                ..AgentDirectoryQuery::default()
            })
            .expect("active list")
            .entries
            .len(),
        2
    );
    assert_eq!(
        service.list(&AgentDirectoryQuery {
            limit: 0,
            ..AgentDirectoryQuery::default()
        }),
        Err(AgentRuntimeError::InvalidRequest)
    );
}

#[test]
fn get_resolves_full_id_unique_prefix_and_exact_title() {
    let (service, agents) = service();
    assert_eq!(service.get("agent-a").expect("full id").agent.id, "agent-a");
    assert_eq!(service.get("agent-b").expect("prefix").agent.id, "agent-b");
    assert_eq!(service.get("Alpha").expect("title").agent.id, "agent-a");

    let mut duplicate = agent("another", "wks-one", "Alpha", false);
    duplicate.labels.clear();
    agents.upsert(&duplicate).expect("fixture insert");
    assert!(matches!(
        service.get("Alpha"),
        Err(AgentRuntimeError::Ambiguous(_))
    ));
    assert!(matches!(
        service.get("missing"),
        Err(AgentRuntimeError::NotFound(_))
    ));
}

#[test]
fn metadata_attention_and_detach_mutations_match_paseo_rules() {
    let (service, agents) = service();
    agents
        .update("agent-a", &|current| {
            let mut next = current.clone();
            next.requires_attention = true;
            next.attention_reason = Some(AgentAttentionReason::Finished);
            next.attention_timestamp = Some("then".to_owned());
            next.labels
                .insert(PARENT_AGENT_ID_LABEL.to_owned(), "parent".to_owned());
            next.labels
                .insert("paseo.open-agent-tab.client".to_owned(), "true".to_owned());
            next
        })
        .expect("fixture update");

    let labels = BTreeMap::from([("new".to_owned(), "value".to_owned())]);
    let updated = service
        .update("agent-a", Some("  Renamed  "), Some(&labels), "later")
        .expect("update should succeed");
    assert_eq!(updated.title.as_deref(), Some("Renamed"));
    assert_eq!(updated.labels.get("new").map(String::as_str), Some("value"));
    assert_eq!(
        updated.labels.get("team").map(String::as_str),
        Some("server")
    );
    assert_eq!(
        updated
            .labels
            .get(PARENT_AGENT_ID_LABEL)
            .map(String::as_str),
        Some("parent")
    );
    assert_eq!(
        service.update("agent-a", Some("  "), Some(&BTreeMap::new()), "later"),
        Err(AgentRuntimeError::InvalidRequest)
    );

    let cleared = service
        .clear_attention(&["agent-a".to_owned()], "clear-time")
        .expect("attention should clear");
    assert!(!cleared[0].requires_attention);
    let detached = service
        .detach("agent-a", "detach-time")
        .expect("detach should succeed");
    assert!(!detached.labels.contains_key(PARENT_AGENT_ID_LABEL));
    assert!(
        detached
            .labels
            .keys()
            .all(|label| !label.starts_with(OPEN_AGENT_TAB_LABEL_PREFIX))
    );

    agents
        .update("agent-a", &|current| {
            let mut next = current.clone();
            next.labels
                .insert("paseo.open-agent-tab.orphan".to_owned(), "true".to_owned());
            next
        })
        .expect("fixture update");
    let already_detached = service
        .detach("agent-a", "must-not-change")
        .expect("already detached should be accepted");
    assert_eq!(already_detached.updated_at, "detach-time");
    assert_eq!(
        already_detached
            .labels
            .get("paseo.open-agent-tab.orphan")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn archive_cascades_same_workspace_children_and_detaches_handoffs() {
    let (service, agents) = service();
    let mut child = agent("child", "wks-one", "Child", false);
    child
        .labels
        .insert(PARENT_AGENT_ID_LABEL.to_owned(), "agent-a".to_owned());
    agents.upsert(&child).expect("fixture insert");
    let mut handoff = agent("handoff", "wks-two", "Handoff", false);
    handoff
        .labels
        .insert(PARENT_AGENT_ID_LABEL.to_owned(), "agent-a".to_owned());
    agents.upsert(&handoff).expect("fixture insert");

    let archived = service
        .archive("agent-a", "2026-09-22T00:00:00.000Z")
        .expect("archive should succeed");
    assert_eq!(
        archived.archived_at.as_deref(),
        Some("2026-09-22T00:00:00.000Z")
    );
    assert_eq!(archived.updated_at, "2026-09-20T11:00:00.000Z");
    assert!(
        agents
            .get("child")
            .expect("read")
            .expect("child")
            .archived_at
            .is_some()
    );
    let handoff = agents.get("handoff").expect("read").expect("handoff");
    assert!(handoff.archived_at.is_none());
    assert!(!handoff.labels.contains_key(PARENT_AGENT_ID_LABEL));
}

#[test]
fn repeated_archive_does_not_start_a_new_child_cascade() {
    let (service, agents) = service();
    agents
        .update("agent-a", &|current| {
            let mut next = current.clone();
            next.archived_at = Some("already".to_owned());
            next
        })
        .expect("fixture update");
    let mut late_child = agent("late-child", "wks-one", "Late child", false);
    late_child
        .labels
        .insert(PARENT_AGENT_ID_LABEL.to_owned(), "agent-a".to_owned());
    agents.upsert(&late_child).expect("fixture insert");

    let archived = service
        .archive("agent-a", "later")
        .expect("repeated archive should succeed");
    assert_eq!(archived.archived_at.as_deref(), Some("already"));
    assert!(
        agents
            .get("late-child")
            .expect("registry read")
            .expect("late child")
            .archived_at
            .is_none()
    );
}

#[test]
fn delete_is_permanent_and_missing_identity_is_explicit() {
    let (service, _) = service();
    service.delete("agent-a").expect("delete should succeed");
    assert!(matches!(
        service.get("agent-a"),
        Err(AgentRuntimeError::NotFound(_))
    ));
    assert_eq!(
        service.delete("agent-a"),
        Err(AgentRuntimeError::NotFound("agent-a".to_owned()))
    );
}

fn default_query() -> AgentDirectoryQuery {
    AgentDirectoryQuery {
        limit: 200,
        ..AgentDirectoryQuery::default()
    }
}

fn service() -> (AgentRuntimeDirectory, Agents) {
    let agents = Agents::default();
    agents
        .upsert(&agent("agent-a", "wks-one", "Alpha", false))
        .expect("fixture insert");
    agents
        .upsert(&agent("agent-b", "wks-one", "Beta", false))
        .expect("fixture insert");
    let projects = Projects::default();
    projects
        .upsert(&project("prj-one", "key-one"))
        .expect("fixture insert");
    let workspaces = Workspaces::default();
    workspaces
        .upsert(
            &workspace("wks-one", "prj-one"),
            WorkspaceMutationContext::default(),
        )
        .expect("fixture insert");
    workspaces
        .upsert(
            &workspace("wks-two", "prj-one"),
            WorkspaceMutationContext::default(),
        )
        .expect("fixture insert");
    (
        AgentRuntimeDirectory::new(
            Box::new(agents.clone()),
            Box::new(workspaces),
            Box::new(projects),
        ),
        agents,
    )
}

fn agent(id: &str, workspace_id: &str, title: &str, internal: bool) -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: id.to_owned(),
        provider: "codex".to_owned(),
        cwd: "/repo".to_owned(),
        workspace_id: Some(workspace_id.to_owned()),
        created_at: format!("2026-09-20T10:00:0{}.000Z", i32::from(id == "agent-b")),
        updated_at: format!("2026-09-20T11:00:0{}.000Z", i32::from(id == "agent-b")),
        last_activity_at: None,
        last_user_message_at: None,
        title: Some(title.to_owned()),
        labels: BTreeMap::from([("team".to_owned(), "server".to_owned())]),
        last_status: AgentRuntimeStatus::Idle,
        last_mode_id: None,
        config: None,
        runtime_info: None,
        features: Vec::new(),
        persistence: None,
        last_error: None,
        requires_attention: false,
        attention_reason: None,
        attention_timestamp: None,
        internal,
        archived_at: None,
        owner: None,
    }
}

fn project(id: &str, key: &str) -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: id.to_owned(),
        root_path: "/repo".to_owned(),
        kind: PersistedProjectKind::Git,
        display_name: "Project".to_owned(),
        project_key: Some(key.to_owned()),
        custom_name: None,
        custom_icon_revision: None,
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:00:00.000Z".to_owned(),
        archived_at: None,
    }
}

fn workspace(id: &str, project_id: &str) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: project_id.to_owned(),
        cwd: "/repo".to_owned(),
        kind: PersistedWorkspaceKind::LocalCheckout,
        display_name: "Main".to_owned(),
        title: None,
        branch: Some("main".to_owned()),
        worktree_root: Some("/repo".to_owned()),
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: None,
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:00:00.000Z".to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}
