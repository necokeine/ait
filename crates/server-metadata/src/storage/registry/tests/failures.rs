use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ports::registry::{
    ProjectRegistry, RegistryError, WorkspaceMutationContext, WorkspaceRegistry,
};

use super::super::{FileBackedProjectRegistry, FileBackedWorkspaceRegistry};
use super::fixtures::{input, project, workspace};

#[test]
fn failed_writes_preserve_disk_cache_and_observers_and_allow_retry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspaces.json");
    let mut registry = FileBackedWorkspaceRegistry::new(path.clone());
    let should_fail = Arc::new(AtomicBool::new(false));
    let flag = should_fail.clone();
    let original = registry.file.writer.clone();
    Arc::get_mut(&mut registry.file).unwrap().writer = Arc::new(move |path, bytes| {
        if flag.load(Ordering::SeqCst) {
            Err(RegistryError::Io)
        } else {
            original(path, bytes)
        }
    });
    registry
        .upsert(&workspace("one"), WorkspaceMutationContext::default())
        .unwrap();
    let previous = std::fs::read(&path).unwrap();
    let published = Arc::new(Mutex::new(Vec::new()));
    let captured = published.clone();
    let _subscription = registry.subscribe_to_mutations(Arc::new(move |event| {
        captured.lock().unwrap().push(event.clone());
        Ok(())
    }));
    should_fail.store(true, Ordering::SeqCst);
    assert_eq!(
        registry.update("one", &|r| {
            let mut next = r.clone();
            next.title = Some("uncommitted".into());
            next
        }),
        Err(RegistryError::Io)
    );
    assert_eq!(registry.remove("one"), Err(RegistryError::Io));
    assert_eq!(registry.list().unwrap(), [workspace("one")]);
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert!(published.lock().unwrap().is_empty());
    should_fail.store(false, Ordering::SeqCst);
    registry.remove("one").unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert_eq!(published.lock().unwrap().len(), 1);
}

#[test]
fn malformed_files_and_invalid_programmatic_records_are_never_silently_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspaces.json");
    std::fs::write(&path, b"[{broken").unwrap();
    let registry = FileBackedWorkspaceRegistry::new(path.clone());
    assert!(registry.exists_on_disk());
    assert_eq!(registry.initialize(), Err(RegistryError::InvalidFile));
    assert_eq!(
        registry.upsert(&workspace("one"), WorkspaceMutationContext::default()),
        Err(RegistryError::InvalidFile)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"[{broken");
    std::fs::write(&path, b"[]").unwrap();
    registry.initialize().unwrap();
    let mut invalid = workspace("one");
    invalid.untrusted_source = Some(
        crate::model::registry::UntrustedWorkspaceSource::ChangeRequest {
            forge: "github".into(),
            number: 0,
            head_repository: "example/repo".into(),
        },
    );
    assert_eq!(
        registry.upsert(&invalid, WorkspaceMutationContext::default()),
        Err(RegistryError::InvalidRecord)
    );
    assert!(registry.list().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), b"[]");
    let directory = FileBackedProjectRegistry::new(temp.path().to_owned());
    assert_eq!(directory.list(), Err(RegistryError::Io));
    let parent_file = temp.path().join("not-a-directory");
    std::fs::write(&parent_file, b"retained").unwrap();
    let blocked = FileBackedProjectRegistry::new(parent_file.join("projects.json"));
    assert_eq!(blocked.upsert(&project("one")), Err(RegistryError::Io));
    assert_eq!(std::fs::read(&parent_file).unwrap(), b"retained");
}

#[test]
fn observer_failure_semantics_and_freeze_match_registry_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let projects = FileBackedProjectRegistry::new(temp.path().join("projects.json"));
    let project_subscription =
        projects.subscribe_to_mutations(Arc::new(|_| Err(RegistryError::Io)));
    assert_eq!(
        projects.upsert(&project("committed")),
        Err(RegistryError::Observer)
    );
    assert!(projects.get("committed").unwrap().is_some());
    drop(project_subscription);
    projects.upsert(&project("after-unsubscribe")).unwrap();
    let path = temp.path().join("workspaces.json");
    let workspaces = FileBackedWorkspaceRegistry::new(path.clone());
    let _subscription = workspaces.subscribe_to_mutations(Arc::new(|_| Err(RegistryError::Io)));
    workspaces
        .upsert(&workspace("committed"), WorkspaceMutationContext::default())
        .unwrap();
    workspaces.block_all_mutations_until_restart().unwrap();
    assert_eq!(workspaces.remove("missing"), Err(RegistryError::Frozen));
    assert!(workspaces.get("committed").unwrap().is_some());
    let reopened = FileBackedWorkspaceRegistry::new(path);
    reopened.remove("committed").unwrap();
    assert!(reopened.list().unwrap().is_empty());
}

#[test]
fn allocation_retries_collisions_and_loaded_arrays_keep_map_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projects.json");
    let mut first = project("legacy-b");
    first.created_at = "2020-01-01".into();
    let mut second = project("legacy-a");
    second.created_at = "2020-01-01".into();
    let mut replacement = first.clone();
    replacement.custom_name = Some("last wins".into());
    std::fs::write(
        &path,
        serde_json::to_vec(&[first, second.clone(), replacement.clone()]).unwrap(),
    )
    .unwrap();
    let mut registry = FileBackedProjectRegistry::new(path);
    assert_eq!(registry.list().unwrap(), [replacement, second.clone()]);
    assert_eq!(
        registry
            .get_or_create_active_by_root(&input("/repo"))
            .unwrap(),
        second
    );
    let candidates = Mutex::new(vec!["fresh", "legacy-b"]);
    registry.id_factory =
        Arc::new(move || Ok(candidates.lock().unwrap().pop().unwrap().to_owned()));
    let new = registry
        .get_or_create_active_by_root(&input("/another"))
        .unwrap();
    assert_eq!(new.project_id, "fresh");
}

#[test]
fn concurrent_updates_and_archive_cannot_drop_the_latest_name() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = FileBackedProjectRegistry::new(temp.path().join("projects.json"));
    registry.upsert(&project("one")).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let receiver = Mutex::new(release_rx);
    let original = registry.file.writer.clone();
    let pause = AtomicBool::new(true);
    Arc::get_mut(&mut registry.file).unwrap().writer = Arc::new(move |path, bytes| {
        if pause.swap(false, Ordering::SeqCst) {
            started_tx.send(()).unwrap();
            receiver
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        original(path, bytes)
    });
    std::thread::scope(|scope| {
        let registry = &registry;
        let update = scope.spawn(move || {
            registry
                .update("one", &|r| {
                    let mut next = r.clone();
                    next.custom_name = Some("Kept".into());
                    next
                })
                .unwrap()
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let archive = scope.spawn(move || registry.archive("one", "archived").unwrap());
        release_tx.send(()).unwrap();
        update.join().unwrap();
        archive.join().unwrap();
    });
    let saved = registry.get("one").unwrap().unwrap();
    assert_eq!(saved.display_name(), "Kept");
    assert_eq!(saved.archived_at.as_deref(), Some("archived"));
}
