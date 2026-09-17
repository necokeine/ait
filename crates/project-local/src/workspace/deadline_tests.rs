//! Faults are injected at OS boundaries, but assertions use only public port results.
use super::*;
use crate::DocumentsProjectDirectory;
use ait_ports::ProjectDirectoryCreator;
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

fn expire_at(point: &'static str) -> OperationOptions {
    let mut options = OperationOptions::default();
    let elapsed = options.elapsed_ms.clone();
    options.probe = Some(Arc::new(move |name, _| {
        if name == point {
            elapsed.fetch_add(31_000, Ordering::SeqCst);
        }
    }));
    options
}

fn assert_timeout(failure: &DomainError, code: ErrorCode, retryable: bool) {
    assert_eq!(failure.code, code, "{failure:?}");
    assert_eq!(failure.retryable, retryable, "{failure:?}");
    assert_eq!(failure.details.as_ref().unwrap().0["reason"], "timeout");
}

fn assert_retained(failure: &DomainError, path: &Path, state: &str) {
    let paths = failure.details.as_ref().unwrap().0["retained_paths"]
        .as_array()
        .unwrap();
    assert!(
        paths
            .iter()
            .any(|entry| entry["path"] == path.to_str().unwrap() && entry["state"] == state),
        "{failure:?}"
    );
    assert!(
        failure.message.contains(path.to_str().unwrap()),
        "API message must preserve the path"
    );
    assert!(failure.message.contains(state));
}

async fn repository() -> (tempfile::TempDir, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let adapter = LocalProjectWorkspace::default();
    let root = adapter.prepare_git_root(temp.path(), None).await.unwrap();
    let head = adapter.ensure_git_head(&root).await.unwrap();
    (temp, root, head)
}

#[tokio::test]
async fn every_public_operation_maps_admission_timeout_to_its_own_responsibility() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let options = OperationOptions {
        timeout: Duration::ZERO,
        ..OperationOptions::default()
    };
    let adapter = LocalProjectWorkspace {
        options: options.clone(),
        ..LocalProjectWorkspace::default()
    };
    let workspace: &dyn ProjectWorkspace = &adapter;
    let target = root.join(".ait/session");
    for (failure, expected) in [
        (
            workspace.prepare_git_root(root, None).await.unwrap_err(),
            ErrorCode::ProjectGitInitFailed,
        ),
        (
            workspace.verify_git_root(root).await.unwrap_err(),
            ErrorCode::ProjectGitInitFailed,
        ),
        (
            workspace.ensure_git_head(root).await.unwrap_err(),
            ErrorCode::ProjectGitHeadUnavailable,
        ),
        (
            workspace.git_head(root).await.unwrap_err(),
            ErrorCode::ProjectGitHeadUnavailable,
        ),
        (
            workspace.clean_baseline(root).await.unwrap_err(),
            ErrorCode::ProjectGitHeadUnavailable,
        ),
        (
            workspace.symbolic_head(root).await.unwrap_err(),
            ErrorCode::ProjectGitHeadUnavailable,
        ),
        (
            workspace.git_dir(root).await.unwrap_err(),
            ErrorCode::ProjectGitHeadUnavailable,
        ),
        (
            workspace.acquire_lease(root).await.err().unwrap(),
            ErrorCode::ProjectWorkspaceBusy,
        ),
        (
            workspace
                .ensure_session_worktree(root, &target, "unused", None)
                .await
                .unwrap_err(),
            ErrorCode::ProjectGitInitFailed,
        ),
        (
            workspace.path_facts(root, &target).await.unwrap_err(),
            ErrorCode::ProjectPathNotFound,
        ),
    ] {
        assert_timeout(&failure, expected, true);
        assert!(!failure.details.unwrap().0.contains_key("retained_paths"));
    }
    let path = root.to_owned();
    let mut creator = DocumentsProjectDirectory::with_resolver(move || Some(path.clone()));
    creator.options = options;
    let failure = ProjectDirectoryCreator::create_workdir(&creator, "new")
        .await
        .unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectDirectoryCreationFailed, true);
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

#[tokio::test]
async fn lease_phases_share_one_absolute_budget() {
    let (_temp, root, _) = repository().await;
    let mut options = OperationOptions::default();
    let elapsed = options.elapsed_ms.clone();
    options.probe = Some(Arc::new(move |name, context| match name {
        "after_canonicalize" => {
            elapsed.fetch_add(10_000, Ordering::SeqCst);
        }
        "after_lease_queue" => {
            assert!(context.remaining() <= Duration::from_secs(20));
            elapsed.fetch_add(21_000, Ordering::SeqCst);
        }
        _ => {}
    }));
    let adapter = LocalProjectWorkspace {
        options,
        ..LocalProjectWorkspace::default()
    };
    let failure = adapter.acquire_lease(&root).await.err().unwrap();
    assert_timeout(&failure, ErrorCode::ProjectWorkspaceBusy, true);
    assert!(
        !root.join(".git/ait/locks").exists(),
        "expired calls cannot enter the file-lock phase"
    );
    let _next = LocalProjectWorkspace::default()
        .acquire_lease(&root)
        .await
        .unwrap();
}

#[tokio::test]
async fn waiting_on_a_held_lease_consumes_only_the_remaining_budget() {
    let (_temp, root, _) = repository().await;
    let owner = LocalProjectWorkspace::default();
    let held = owner.acquire_lease(&root).await.unwrap();
    let mut contender = owner.clone();
    let elapsed = contender.options.elapsed_ms.clone();
    contender.options.probe = Some(Arc::new(move |name, _| {
        if name == "after_canonicalize" {
            elapsed.fetch_add(29_900, Ordering::SeqCst);
        }
    }));
    let started = Instant::now();
    let failure = contender.acquire_lease(&root).await.err().unwrap();
    assert_timeout(&failure, ErrorCode::ProjectWorkspaceBusy, true);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "queue reset the deadline"
    );
    drop(held);
    let _next = owner.acquire_lease(&root).await.unwrap();
}

#[tokio::test]
async fn capacity_queue_timeout_uses_the_public_operation_code() {
    let temp = tempfile::tempdir().unwrap();
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let adapter = LocalProjectWorkspace {
        options: OperationOptions {
            timeout: Duration::from_millis(50),
            permits: Some(permits.clone()),
            ..OperationOptions::default()
        },
        ..LocalProjectWorkspace::default()
    };
    let failure = adapter
        .path_facts(temp.path(), temp.path())
        .await
        .unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectPathNotFound, true);
    permits.add_permits(1);
    adapter.path_facts(temp.path(), temp.path()).await.unwrap();
}

#[tokio::test]
async fn implicit_worktree_lease_does_not_reset_the_outer_deadline() {
    let (_temp, root, head) = repository().await;
    let mut options = OperationOptions::default();
    let elapsed = options.elapsed_ms.clone();
    options.probe = Some(Arc::new(move |name, context| {
        if name == "lease_acquired" {
            elapsed.fetch_add(20_000, Ordering::SeqCst);
        }
        if name == "before_worktree" {
            assert!(context.remaining() <= Duration::from_secs(10));
            elapsed.fetch_add(11_000, Ordering::SeqCst);
        }
    }));
    let adapter = LocalProjectWorkspace {
        options,
        ..LocalProjectWorkspace::default()
    };
    let target = root.join(".ait/session");
    let failure = adapter
        .ensure_session_worktree(&root, &target, &head, None)
        .await
        .unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectGitInitFailed, true);
    assert!(!target.exists());
    let _next = LocalProjectWorkspace::default()
        .acquire_lease(&root)
        .await
        .unwrap();
}

#[tokio::test]
async fn directory_completed_across_deadline_is_reported_and_never_reused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let resolver_root = root.clone();
    let mut creator = DocumentsProjectDirectory::with_resolver(move || Some(resolver_root.clone()));
    creator.options = expire_at("directory_created");
    let failure = ProjectDirectoryCreator::create_workdir(&creator, "retained")
        .await
        .unwrap_err();
    let path = root.join("retained");
    assert_timeout(&failure, ErrorCode::ProjectDirectoryCreationFailed, false);
    assert_retained(&failure, &path, "directory_created");
    assert!(path.is_dir());
    fs::write(path.join("member-data"), "keep").unwrap();
    let retry = ProjectDirectoryCreator::create_workdir(&creator, "retained")
        .await
        .unwrap_err();
    assert_eq!(retry.code, ErrorCode::ProjectPathAlreadyExists);
    assert!(!retry.retryable);
    assert_eq!(
        fs::read_to_string(path.join("member-data")).unwrap(),
        "keep"
    );
}

#[tokio::test]
async fn directory_timeout_reaches_application_with_path_and_without_registration() {
    use ait_ports::{ControlFilter, ControlRecordKind, ControlStore};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let resolver_root = root.clone();
    let mut creator = DocumentsProjectDirectory::with_resolver(move || Some(resolver_root.clone()));
    creator.options = expire_at("directory_created");
    let store = Arc::new(ait_storage_sqlite::SqliteControlStore::in_memory().unwrap());
    let service = ait_application::LocalControlService::new(
        Arc::new(LocalProjectWorkspace::default()),
        store.clone(),
    )
    .with_project_directory_creator(Arc::new(creator));
    let response = service
        .execute(ait_contracts::Command::RegisterProject {
            id: "project".into(),
            name: "retained".into(),
            workdir: None,
            repo_url: None,
        })
        .await;
    assert!(!response.ok);
    let failure = response.error.unwrap();
    assert_eq!(failure.code, ErrorCode::ProjectDirectoryCreationFailed);
    assert!(!failure.retryable);
    assert!(
        failure
            .message
            .contains(root.join("retained").to_str().unwrap())
    );
    assert!(failure.message.contains("directory_created"));
    assert!(
        store
            .read(&[
                ControlFilter::all(ControlRecordKind::Project),
                ControlFilter::all(ControlRecordKind::Message)
            ])
            .await
            .unwrap()
            .records
            .is_empty()
    );
    assert!(root.join("retained").is_dir());
}

#[tokio::test]
async fn git_initialization_and_commit_crossing_deadline_report_retained_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let adapter = LocalProjectWorkspace {
        options: expire_at("git_initialized"),
        ..LocalProjectWorkspace::default()
    };
    let failure = adapter.prepare_git_root(&root, None).await.unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectGitInitFailed, false);
    assert_retained(&failure, &root, "git_initialized");
    assert!(root.join(".git").is_dir());
    let adapter = LocalProjectWorkspace {
        options: expire_at("initial_commit_created"),
        ..LocalProjectWorkspace::default()
    };
    let failure = adapter.ensure_git_head(&root).await.unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectGitHeadUnavailable, false);
    assert_retained(&failure, &root, "initial_commit_created");
    assert!(
        LocalProjectWorkspace::default()
            .git_head(&root)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn worktree_creation_and_population_crossing_deadline_report_actual_state() {
    for point in ["worktree_created", "worktree_populated"] {
        let (_temp, root, _) = repository().await;
        fs::write(root.join("tracked"), "baseline content").unwrap();
        git(&root, &["add", "tracked"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@localhost",
                "commit",
                "--no-gpg-sign",
                "-m",
                "fixture",
            ],
        );
        let normal = LocalProjectWorkspace::default();
        let head = normal.git_head(&root).await.unwrap().unwrap();
        let adapter = LocalProjectWorkspace {
            options: expire_at(point),
            ..LocalProjectWorkspace::default()
        };
        let target = root.join(".ait/session");
        let failure = adapter
            .ensure_session_worktree(&root, &target, &head, None)
            .await
            .unwrap_err();
        assert_timeout(&failure, ErrorCode::ProjectGitInitFailed, false);
        assert_retained(&failure, &target, point);
        assert!(target.join(".git").is_file());
        assert_eq!(
            target.join("tracked").exists(),
            point == "worktree_populated"
        );
        // Retrying an existing worktree must never populate or reset it.
        fs::write(target.join("member-data"), "keep").unwrap();
        assert!(
            !normal
                .ensure_session_worktree(&root, &target, &head, None)
                .await
                .unwrap()
        );
        assert_eq!(
            fs::read_to_string(target.join("member-data")).unwrap(),
            "keep"
        );
        assert_eq!(
            target.join("tracked").exists(),
            point == "worktree_populated"
        );
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn running_git_timeout_is_mapped_at_the_public_port() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("stalled-git");
    fs::write(&script, "#!/bin/sh\nexec sleep 20\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = LocalProjectWorkspace {
        options: OperationOptions {
            timeout: Duration::from_millis(150),
            git_program: Some(script),
            ..OperationOptions::default()
        },
        ..LocalProjectWorkspace::default()
    };
    let start = Instant::now();
    let failure = adapter.git_head(temp.path()).await.unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectGitHeadUnavailable, true);
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[cfg(unix)]
#[tokio::test]
async fn git_timeout_after_worktree_side_effect_reports_uncertain_creation_state() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, root, head) = repository().await;
    let script = temp.path().join("stall-after-add");
    fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "case \" $* \" in\n",
            "  *\" worktree add \"*) git \"$@\" || exit $?; exec sleep 20;;\n",
            "  *) exec git \"$@\";;\n",
            "esac\n",
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = LocalProjectWorkspace {
        options: OperationOptions {
            timeout: Duration::from_secs(2),
            git_program: Some(script),
            ..OperationOptions::default()
        },
        ..LocalProjectWorkspace::default()
    };
    let target = root.join(".ait/session");
    let started = Instant::now();
    let failure = adapter
        .ensure_session_worktree(&root, &target, &head, None)
        .await
        .unwrap_err();
    assert_timeout(&failure, ErrorCode::ProjectGitInitFailed, false);
    assert_retained(&failure, &target, "worktree_creation_started");
    assert!(
        target.join(".git").is_file(),
        "Git really created the linked worktree before being killed"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    let _next = LocalProjectWorkspace::default()
        .acquire_lease(&root)
        .await
        .unwrap();
}

#[tokio::test]
async fn dropping_public_worktree_future_keeps_its_lease_until_worker_drains() {
    let (_temp, root, head) = repository().await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let channels = Mutex::new(Some((started_tx, release_rx, finished_tx)));
    let adapter = LocalProjectWorkspace {
        options: OperationOptions {
            probe: Some(Arc::new(move |name, context| {
                if name == "before_worktree" {
                    let (started, release, finished) = channels.lock().unwrap().take().unwrap();
                    started.send(()).unwrap();
                    release.recv().unwrap();
                    assert_eq!(context.check().unwrap_err().code, ErrorCode::RunCancelled);
                    finished.send(()).unwrap();
                }
            })),
            ..OperationOptions::default()
        },
        ..LocalProjectWorkspace::default()
    };
    let same_adapter = adapter.clone();
    let task_root = root.clone();
    let task = tokio::spawn(async move {
        adapter
            .ensure_session_worktree(&task_root, &task_root.join(".ait/session"), &head, None)
            .await
    });
    started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let contender = LocalProjectWorkspace::default();
    let busy = contender.acquire_lease(&root).await.err().unwrap();
    assert_eq!(busy.code, ErrorCode::ProjectWorkspaceBusy);
    assert!(busy.retryable);
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    // The canonical queue cannot admit this request until the worker releases its lease.
    let _next = same_adapter.acquire_lease(&root).await.unwrap();
    assert!(!root.join(".ait/session").exists());
}

#[test]
fn saturated_tokio_blocking_queue_cannot_extend_public_deadline_or_start_late_io() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.await.unwrap();
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let adapter = LocalProjectWorkspace {
            options: OperationOptions {
                timeout: Duration::from_millis(50),
                permits: Some(permits.clone()),
                ..OperationOptions::default()
            },
            ..LocalProjectWorkspace::default()
        };
        // Bound the assertion too, so a broken adapter cannot hang the test.
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            adapter.prepare_git_root(temp.path(), None),
        )
        .await;
        assert_eq!(
            permits.available_permits(),
            1,
            "timeout must release queued capacity before the pool unblocks"
        );
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        let failure = result
            .expect("Tokio queue reset or escaped the deadline")
            .unwrap_err();
        assert_timeout(&failure, ErrorCode::ProjectGitInitFailed, true);
        assert!(
            !temp.path().join(".git").exists(),
            "a timed-out queued job must never start mutation"
        );
    });
}

#[test]
fn workspace_lock_probe_process() {
    let Some(path) = std::env::var_os("AIT_QUEUED_LEASE_PROBE_PATH") else {
        return;
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    assert_eq!(
        file.try_lock().is_ok(),
        std::env::var("AIT_QUEUED_LEASE_PROBE_FREE").unwrap() == "true"
    );
}

fn assert_process_lock_available(root: &Path, available: bool) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::deadline_tests::workspace_lock_probe_process",
        ])
        .env(
            "AIT_QUEUED_LEASE_PROBE_PATH",
            root.join(".git/ait/locks/workspace-write.lock"),
        )
        .env("AIT_QUEUED_LEASE_PROBE_FREE", available.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "independent lock probe failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn dropping_a_queued_public_worktree_future_releases_resources_before_pool_unblocks() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_temp, root, head) = repository().await;
        let duplicate = Arc::new(Mutex::new(None));
        let mut adapter = LocalProjectWorkspace {
            lease_duplicate: Some(duplicate.clone()),
            ..LocalProjectWorkspace::default()
        };
        let lease = adapter.acquire_lease(&root).await.unwrap();
        let weak_lease = Arc::downgrade(&lease);
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let queued = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = queued.clone();
        adapter.options.permits = Some(permits.clone());
        adapter.options.probe = Some(Arc::new(move |point, _| {
            if point == "worker_queued" {
                observed.store(true, Ordering::SeqCst);
            }
        }));

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            // Dropping the sender also releases this on assertion failure.
            let _ = release_rx.recv();
        });
        started_rx.await.unwrap();
        let target = root.join(".ait/session");
        let mut future =
            adapter.ensure_session_worktree(&root, &target, &head, Some(lease.clone()));
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(
            queued.load(Ordering::SeqCst),
            "the public worker must already be in Tokio's queue"
        );
        assert_eq!(permits.available_permits(), 0);
        assert_eq!(
            Arc::strong_count(&lease),
            2,
            "the queued closure owns a lease clone"
        );
        drop(lease);
        assert_eq!(weak_lease.strong_count(), 1);
        assert_process_lock_available(&root, false);
        drop(future);

        assert!(
            !blocker.is_finished(),
            "the unrelated syscall is still blocking the pool"
        );
        assert_eq!(
            weak_lease.strong_count(),
            0,
            "dropping queued work must release its lease immediately"
        );
        assert_eq!(
            permits.available_permits(),
            1,
            "dropping queued work must return admission immediately"
        );
        assert!(
            duplicate.lock().unwrap().is_some(),
            "a concurrent process can still hold the inherited file description"
        );
        // Check the actual advisory file lock without using the saturated pool.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(".git/ait/locks/workspace-write.lock"))
            .unwrap();
        file.try_lock()
            .expect("cancelled queued work must no longer exclude another lock owner");
        file.unlock().unwrap();
        drop(file);
        assert_process_lock_available(&root, true);
        assert!(!target.exists());

        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        assert!(
            !target.exists(),
            "cancelled queued work must never perform late I/O"
        );
        let _next = adapter.acquire_lease(&root).await.unwrap();
    });
}
