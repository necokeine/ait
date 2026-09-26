//! Paseo workspace-registry concurrent update and archive isolation cases.

use std::sync::Barrier;

use super::*;
use crate::ports::registry::{WorkspaceArchiveContext, WorkspaceMutationContext};

#[test]
fn concurrent_workspace_field_updates_compose_and_survive_reopening() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workspaces.json");
    let registry = FileBackedWorkspaceRegistry::new(path.clone());
    registry
        .upsert(&workspace("one"), WorkspaceMutationContext::default())
        .unwrap();
    let barrier = Barrier::new(3);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            registry
                .update("one", &|record| {
                    let mut next = record.clone();
                    next.title = Some("Concurrent title".to_owned());
                    next
                })
                .unwrap();
        });
        scope.spawn(|| {
            barrier.wait();
            registry
                .update("one", &|record| {
                    let mut next = record.clone();
                    next.pinned_at = Some("Concurrent pin".to_owned());
                    next
                })
                .unwrap();
        });
        barrier.wait();
    });
    let saved = FileBackedWorkspaceRegistry::new(path)
        .get("one")
        .unwrap()
        .unwrap();
    assert_eq!(saved.title.as_deref(), Some("Concurrent title"));
    assert_eq!(saved.pinned_at.as_deref(), Some("Concurrent pin"));
}

#[test]
fn concurrent_workspace_archive_and_title_update_preserve_both_changes() {
    let root = tempfile::tempdir().unwrap();
    let registry = FileBackedWorkspaceRegistry::new(root.path().join("workspaces.json"));
    registry
        .upsert(&workspace("one"), WorkspaceMutationContext::default())
        .unwrap();
    let barrier = Barrier::new(3);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            registry
                .update("one", &|record| {
                    let mut next = record.clone();
                    next.title = Some("Keep title".to_owned());
                    next
                })
                .unwrap();
        });
        scope.spawn(|| {
            barrier.wait();
            registry
                .archive("one", "archived", &WorkspaceArchiveContext::default())
                .unwrap();
        });
        barrier.wait();
    });
    let saved = registry.get("one").unwrap().unwrap();
    assert_eq!(saved.title.as_deref(), Some("Keep title"));
    assert_eq!(saved.archived_at.as_deref(), Some("archived"));
}

#[test]
fn workspace_archive_keeps_same_cwd_sibling_active_and_unmodified() {
    let root = tempfile::tempdir().unwrap();
    let registry = FileBackedWorkspaceRegistry::new(root.path().join("workspaces.json"));
    for id in ["one", "two"] {
        registry
            .upsert(&workspace(id), WorkspaceMutationContext::default())
            .unwrap();
    }
    registry
        .archive(
            "one",
            "archived",
            &WorkspaceArchiveContext {
                auto_archived_change_request_url: Some("https://example.invalid/pr/1".to_owned()),
            },
        )
        .unwrap();
    assert_eq!(registry.get("two").unwrap(), Some(workspace("two")));
    assert_eq!(
        registry
            .get("one")
            .unwrap()
            .unwrap()
            .auto_archived_change_request_url
            .as_deref(),
        Some("https://example.invalid/pr/1")
    );
}

#[test]
fn frozen_workspace_registry_rejects_every_mutation_but_keeps_reads_available() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workspaces.json");
    let registry = FileBackedWorkspaceRegistry::new(path.clone());
    registry
        .upsert(&workspace("one"), WorkspaceMutationContext::default())
        .unwrap();
    let disk = std::fs::read(&path).unwrap();
    registry.block_all_mutations_until_restart().unwrap();
    assert_eq!(
        registry.upsert(&workspace("two"), WorkspaceMutationContext::default()),
        Err(RegistryError::Frozen)
    );
    assert_eq!(
        registry.update("one", &Clone::clone),
        Err(RegistryError::Frozen)
    );
    assert_eq!(
        registry.archive("one", "archived", &WorkspaceArchiveContext::default()),
        Err(RegistryError::Frozen)
    );
    assert_eq!(registry.remove("one"), Err(RegistryError::Frozen));
    assert_eq!(registry.get("one").unwrap(), Some(workspace("one")));
    assert_eq!(std::fs::read(&path).unwrap(), disk);
}
