//! Paseo session.workspaces exact placement, archive, and project lifecycle cases.

use super::*;
use crate::service::directory::WorkspaceCreation;

#[test]
fn opening_a_child_does_not_reuse_an_active_parent_workspace() {
    let directory = directory();
    let child = directory
        .open_workspace("/tmp/alpha/child", "later")
        .unwrap();
    assert_ne!(child.workspace_id, "wks_a");
    assert_eq!(child.cwd, "/tmp/alpha/child");
    assert_eq!(
        directory.workspaces.get("wks_a").unwrap().unwrap().cwd,
        "/tmp/alpha"
    );
    assert_eq!(directory.list_workspaces().unwrap().len(), 2);
}

#[test]
fn opening_a_child_does_not_restore_an_archived_parent_workspace() {
    let directory = directory();
    directory.archive_workspace("wks_a", "archived").unwrap();
    let child = directory
        .open_workspace("/tmp/alpha/child", "later")
        .unwrap();
    assert_ne!(child.workspace_id, "wks_a");
    assert_eq!(
        directory
            .workspaces
            .get("wks_a")
            .unwrap()
            .unwrap()
            .archived_at
            .as_deref(),
        Some("archived")
    );
}

#[test]
fn orphaned_archived_workspace_is_preserved_and_a_fresh_workspace_is_created() {
    let directory = directory();
    directory.archive_workspace("wks_a", "archived").unwrap();
    directory.projects.remove("prj_a").unwrap();
    let opened = directory.open_workspace("/tmp/alpha", "later").unwrap();
    assert_ne!(opened.workspace_id, "wks_a");
    assert_ne!(opened.project_id, "prj_a");
    assert_eq!(
        directory
            .workspaces
            .get("wks_a")
            .unwrap()
            .unwrap()
            .archived_at
            .as_deref(),
        Some("archived")
    );
    assert_eq!(directory.list_workspaces().unwrap().len(), 2);
}

#[test]
fn archived_project_does_not_repurpose_its_archived_workspace() {
    let directory = directory();
    directory.archive_workspace("wks_a", "archived").unwrap();
    directory.projects.archive("prj_a", "archived").unwrap();
    let opened = directory.open_workspace("/tmp/alpha", "later").unwrap();
    assert_ne!(opened.workspace_id, "wks_a");
    assert_ne!(opened.project_id, "prj_a");
    assert!(opened.archived_at.is_none());
    assert_eq!(
        directory
            .projects
            .get("prj_a")
            .unwrap()
            .unwrap()
            .archived_at
            .as_deref(),
        Some("archived")
    );
}

#[test]
fn non_git_workspace_name_is_its_directory_without_a_branch_fallback() {
    let directory = directory();
    let opened = directory
        .open_workspace("/tmp/plain-project", "now")
        .unwrap();
    assert_eq!(opened.kind, PersistedWorkspaceKind::Directory);
    assert_eq!(opened.display_name, "plain-project");
    assert!(opened.branch.is_none());
    assert!(opened.worktree_root.is_none());
    let project = directory.projects.get(&opened.project_id).unwrap().unwrap();
    assert_eq!(project.kind, PersistedProjectKind::NonGit);
    assert_eq!(project.root_path, "/tmp/plain-project");
}

#[test]
fn explicit_parent_project_preserves_exact_child_workspace_placement() {
    let directory = directory();
    let child = directory
        .create_workspace(WorkspaceCreation {
            path: "/tmp/alpha/packages/server",
            title: Some(" Server ".to_owned()),
            project_id: Some("prj_a"),
            workspace_id: Some("wks_child".to_owned()),
            expects_initial_agent: false,
            timestamp: "now",
        })
        .unwrap();
    assert_eq!(child.project_id, "prj_a");
    assert_eq!(child.cwd, "/tmp/alpha/packages/server");
    assert_eq!(child.worktree_root.as_deref(), Some("/tmp/alpha"));
    assert_eq!(child.title.as_deref(), Some("Server"));
    assert_eq!(
        directory.projects.get("prj_a").unwrap().unwrap().root_path,
        "/tmp/alpha"
    );
}

fn seed_plain_record(directory: &Directory, archived: bool) {
    let mut saved = workspace();
    saved.kind = PersistedWorkspaceKind::Directory;
    saved.branch = None;
    saved.worktree_root = None;
    saved.title = Some("Keep title".to_owned());
    saved.labels = Some(vec!["Keep label".to_owned()]);
    saved.archived_at = archived.then(|| "archived".to_owned());
    directory
        .workspaces
        .upsert(&saved, WorkspaceMutationContext::default())
        .unwrap();
    let mut saved = project();
    saved.kind = PersistedProjectKind::NonGit;
    saved.custom_name = Some("Keep project name".to_owned());
    directory.projects.upsert(&saved).unwrap();
}

#[test]
fn opening_reclassifies_an_active_directory_after_it_becomes_git() {
    let directory = directory();
    seed_plain_record(&directory, false);
    let opened = directory.open_workspace("/tmp/alpha", "later").unwrap();
    assert_eq!(opened.workspace_id, "wks_a");
    assert_eq!(opened.kind, PersistedWorkspaceKind::LocalCheckout);
    assert_eq!(opened.branch.as_deref(), Some("main"));
    assert_eq!(opened.title.as_deref(), Some("Keep title"));
    assert_eq!(opened.labels, Some(vec!["Keep label".to_owned()]));
    let project = directory.projects.get("prj_a").unwrap().unwrap();
    assert_eq!(project.kind, PersistedProjectKind::Git);
    assert_eq!(project.custom_name.as_deref(), Some("Keep project name"));
}

#[test]
fn opening_reclassifies_and_restores_an_archived_directory_after_it_becomes_git() {
    let directory = directory();
    seed_plain_record(&directory, true);
    let opened = directory.open_workspace("/tmp/alpha", "later").unwrap();
    assert_eq!(opened.workspace_id, "wks_a");
    assert_eq!(opened.kind, PersistedWorkspaceKind::LocalCheckout);
    assert!(opened.archived_at.is_none());
    assert_eq!(opened.title.as_deref(), Some("Keep title"));
    assert_eq!(opened.labels, Some(vec!["Keep label".to_owned()]));
    assert_eq!(directory.list_workspaces().unwrap().len(), 1);
}

#[test]
fn project_removal_archives_only_its_active_children_and_preserves_other_owners() {
    let directory = directory();
    let mut archived = workspace();
    archived.workspace_id = "wks_archived".to_owned();
    archived.archived_at = Some("old archive".to_owned());
    directory
        .workspaces
        .upsert(&archived, WorkspaceMutationContext::default())
        .unwrap();
    let mut other = workspace();
    other.workspace_id = "wks_other".to_owned();
    other.project_id = "prj_other".to_owned();
    directory
        .workspaces
        .upsert(&other, WorkspaceMutationContext::default())
        .unwrap();
    assert_eq!(
        directory.remove_project("prj_a", "new archive").unwrap(),
        ["wks_a"]
    );
    assert_eq!(
        directory
            .workspaces
            .get("wks_archived")
            .unwrap()
            .unwrap()
            .archived_at
            .as_deref(),
        Some("old archive")
    );
    assert!(
        directory
            .workspaces
            .get("wks_other")
            .unwrap()
            .unwrap()
            .archived_at
            .is_none()
    );
    assert!(directory.projects.get("prj_a").unwrap().is_none());
}

#[test]
fn project_removal_handles_an_already_empty_project() {
    let directory = directory();
    directory.workspaces.remove("wks_a").unwrap();
    assert!(
        directory
            .remove_project("prj_a", "archived")
            .unwrap()
            .is_empty()
    );
    assert!(directory.list_projects().unwrap().is_empty());
    assert!(directory.list_workspaces().unwrap().is_empty());
}

#[test]
fn archived_project_rejects_configuration_reads_and_writes() {
    let directory = directory();
    directory.projects.archive("prj_a", "archived").unwrap();
    assert_eq!(
        directory.read_project_config("/tmp/alpha"),
        Err(DirectoryError::UnknownProject)
    );
    assert_eq!(
        directory.write_project_config("/tmp/alpha", &serde_json::json!({}), None),
        Err(DirectoryError::UnknownProject)
    );
}

#[test]
fn explicit_archived_project_rejects_new_workspace_without_creating_a_record() {
    let directory = directory();
    directory.projects.archive("prj_a", "archived").unwrap();
    let result = directory.create_workspace(WorkspaceCreation {
        path: "/tmp/alpha",
        title: None,
        project_id: Some("prj_a"),
        workspace_id: Some("wks_rejected".to_owned()),
        expects_initial_agent: false,
        timestamp: "now",
    });
    assert_eq!(result, Err(DirectoryError::ArchivedProject));
    assert!(directory.workspaces.get("wks_rejected").unwrap().is_none());
}
