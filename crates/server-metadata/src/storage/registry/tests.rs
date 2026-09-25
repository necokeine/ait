mod failures;
mod fixtures;

use std::sync::{Arc, Mutex};

use crate::ports::registry::{ProjectRegistry, WorkspaceRegistry};

use super::*;
use fixtures::{input, project, workspace};

#[test]
fn project_lifecycle_preserves_records_and_publishes_committed_changes_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projects/projects.json");
    let registry = FileBackedProjectRegistry::new(path.clone());
    registry.initialize().unwrap();
    assert!(!registry.exists_on_disk());
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let reader = registry.clone();
    let listener: crate::ports::registry::MutationListener<
        crate::ports::registry::ProjectMutation,
    > = Arc::new(move |event| {
        assert_eq!(reader.get(&event.project_id).unwrap(), event.project);
        observed.lock().unwrap().push(event.clone());
        Ok(())
    });
    let subscription = registry.subscribe_to_mutations(listener.clone());
    registry.upsert(&project("remote:old")).unwrap();
    registry
        .update("remote:old", &|record| {
            let mut next = record.clone();
            next.custom_name = Some("Custom".to_owned());
            next
        })
        .unwrap();
    registry
        .archive("remote:old", "2026-03-02T00:00:00Z")
        .unwrap();
    registry
        .archive("remote:old", "2026-03-03T00:00:00Z")
        .unwrap();
    registry.archive("missing", "now").unwrap();
    assert!(registry.update("missing", &Clone::clone).unwrap().is_none());
    assert_eq!(events.lock().unwrap().len(), 3);
    let record = registry.get("remote:old").unwrap().unwrap();
    assert_eq!(record.display_name(), "Custom");
    assert_eq!(record.archived_at.as_deref(), Some("2026-03-02T00:00:00Z"));
    assert_eq!(
        FileBackedProjectRegistry::new(path).list().unwrap(),
        [record]
    );
    registry.remove("remote:old").unwrap();
    registry.remove("remote:old").unwrap();
    assert_eq!(events.lock().unwrap().len(), 4);
    drop(subscription); // Holding the original listener Arc must not keep the subscription alive.
    registry.upsert(&project("after-unsubscribe")).unwrap();
    assert_eq!(events.lock().unwrap().len(), 4);
}

#[test]
fn active_root_allocation_serializes_and_keeps_archived_and_legacy_ids() {
    let temp = tempfile::tempdir().unwrap();
    let registry = FileBackedProjectRegistry::new(temp.path().join("projects.json"));
    let allocated = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..20)
            .map(|_| {
                let registry = &registry;
                scope.spawn(move || {
                    registry
                        .get_or_create_active_by_root(&input("/repo/./"))
                        .unwrap()
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(allocated.iter().all(|record| record == &allocated[0]));
    assert_eq!(allocated[0].project_id.len(), 20);
    assert!(allocated[0].project_id.starts_with("prj_"));
    registry
        .archive(&allocated[0].project_id, "2026-03-02T00:00:00Z")
        .unwrap();
    let fresh = registry
        .get_or_create_active_by_root(&input("/repo"))
        .unwrap();
    assert_ne!(fresh.project_id, allocated[0].project_id);
    assert_eq!(registry.list().unwrap().len(), 2);
    let mut legacy = project("remote:legacy");
    legacy.created_at = "2020-01-01T08:00:00+08:00".to_owned();
    legacy.custom_name = Some("Kept".to_owned());
    registry.upsert(&legacy).unwrap();
    let mut refreshed_input = input("/repo");
    refreshed_input.kind = crate::model::registry::PersistedProjectKind::NonGit;
    refreshed_input.project_key = Some("new-key".to_owned());
    refreshed_input.display_name = "Must not replace".to_owned();
    let refreshed = registry
        .get_or_create_active_by_root(&refreshed_input)
        .unwrap();
    assert_eq!(refreshed.project_id, legacy.project_id);
    assert_eq!(refreshed.display_name(), "Kept");
    assert_eq!(refreshed.display_name, legacy.display_name);
    assert_eq!(refreshed.project_key, refreshed_input.project_key);
    assert_eq!(
        registry
            .get_or_create_active_by_root(&refreshed_input)
            .unwrap(),
        refreshed
    );
}

#[test]
fn workspace_lifecycle_keeps_distinct_ids_at_one_cwd_and_archive_context() {
    use crate::ports::registry::{WorkspaceArchiveContext, WorkspaceMutationContext};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspaces.json");
    let registry = FileBackedWorkspaceRegistry::new(path.clone());
    registry.initialize().unwrap();
    assert!(!registry.exists_on_disk());
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let _subscription = registry.subscribe_to_mutations(Arc::new(move |event| {
        captured.lock().unwrap().push(event.clone());
        Ok(())
    }));
    registry
        .upsert(
            &workspace("one"),
            WorkspaceMutationContext {
                expects_initial_agent: Some(true),
            },
        )
        .unwrap();
    registry
        .upsert(
            &workspace("two"),
            WorkspaceMutationContext {
                expects_initial_agent: Some(false),
            },
        )
        .unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
    assert_eq!(events.lock().unwrap()[0].expects_initial_agent, Some(true));
    assert_eq!(events.lock().unwrap()[1].expects_initial_agent, None);
    registry
        .update("one", &|record| {
            let mut next = record.clone();
            next.title = Some("Kept title".to_owned());
            next
        })
        .unwrap();
    registry
        .archive(
            "one",
            "first",
            &WorkspaceArchiveContext {
                auto_archived_change_request_url: Some("https://example.invalid/pr/1".to_owned()),
            },
        )
        .unwrap();
    registry
        .archive("one", "second", &WorkspaceArchiveContext::default())
        .unwrap();
    registry
        .archive(
            "one",
            "third",
            &WorkspaceArchiveContext {
                auto_archived_change_request_url: Some(String::new()),
            },
        )
        .unwrap();
    registry
        .archive("missing", "unused", &WorkspaceArchiveContext::default())
        .unwrap();
    assert!(registry.update("missing", &Clone::clone).unwrap().is_none());
    let stored = registry.get("one").unwrap().unwrap();
    assert_eq!(stored.display_name(), "Kept title");
    assert_eq!(stored.archived_at.as_deref(), Some("third"));
    assert_eq!(
        stored.auto_archived_change_request_url.as_deref(),
        Some("https://example.invalid/pr/1")
    );
    assert_eq!(
        FileBackedWorkspaceRegistry::new(path).list().unwrap(),
        registry.list().unwrap()
    );
    registry.remove("one").unwrap();
    registry.remove("one").unwrap();
    assert_eq!(events.lock().unwrap().len(), 7);
    let generated = generate_workspace_id().unwrap();
    assert_eq!(generated.len(), 20);
    assert!(generated.starts_with("wks_"));
    assert!(generated[4..].bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn lexical_paths_match_windows_namespaces_and_keep_symlink_spellings_distinct() {
    for (left, right) in [
        ("/repo/./sub/../", "/repo"),
        ("", "."),
        ("../../repo", "../x/../../repo"),
        ("C:\\Users\\Paseo\\Repo", "c:/users/paseo/repo/."),
        ("\\\\?\\C:\\Repo", "c:/repo/"),
        ("\\\\?\\UNC\\Server\\Share\\Repo", "\\\\server\\share\\repo"),
    ] {
        assert!(super::paths::equivalent(left, right), "{left} != {right}");
    }
    for (left, right) in [
        ("/repo", "/Repo"),
        ("/target", "/symlink"),
        ("/", "."),
        ("a\\b", "a/b"),
    ] {
        assert!(!super::paths::equivalent(left, right));
    }
    let temp = tempfile::tempdir().unwrap();
    let registry = FileBackedProjectRegistry::new(temp.path().join("projects.json"));
    let first = registry
        .get_or_create_active_by_root(&input("C:\\Users\\Paseo\\Repo"))
        .unwrap();
    assert_eq!(
        first,
        registry
            .get_or_create_active_by_root(&input("c:/users/paseo/repo/."))
            .unwrap()
    );
    assert_ne!(
        registry
            .get_or_create_active_by_root(&input("/target"))
            .unwrap()
            .project_id,
        registry
            .get_or_create_active_by_root(&input("/symlink"))
            .unwrap()
            .project_id
    );
}
