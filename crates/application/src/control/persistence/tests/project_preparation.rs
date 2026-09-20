//! Real Git faults around the one-time preparation and database CAS boundary.
use super::*;
use crate::control::LocalControlService;
use ait_domain::ErrorCode;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::SystemTime;

fn git(path: &Path, arguments: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn command(path: &Path) -> Command {
    Command::RegisterProject {
        id: "p".into(),
        name: "Project".into(),
        workdir: Some(path.display().to_string()),
        repo_url: None,
    }
}

type Files = BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)>;

// Include mtimes as well as contents: repeated Git init/reset can rewrite a file
// with identical bytes. Reading Git metadata must leave both unchanged.
fn snapshot(path: &Path) -> Files {
    fn collect(root: &Path, path: &Path, files: &mut Files) {
        let metadata = path.metadata().unwrap();
        files.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (
                metadata.is_file().then(|| std::fs::read(path).unwrap()),
                metadata.modified().unwrap(),
            ),
        );
        if metadata.is_dir() {
            for child in std::fs::read_dir(path).unwrap() {
                collect(root, &child.unwrap().path(), files);
            }
        }
    }
    let mut files = Files::new();
    collect(path, path, &mut files);
    files
}

async fn assert_no_records_or_events(store: &Probe) {
    let after = store
        .inner
        .read(&[
            ControlFilter::all(Kind::Project),
            ControlFilter::all(Kind::Message),
            ControlFilter::all(Kind::Session),
            ControlFilter::all(Kind::Agent),
            ControlFilter::all(Kind::Provider),
        ])
        .await
        .unwrap();
    assert_eq!(after.revision, 0);
    assert!(after.records.is_empty());
    assert!(store.replay(0, 100).await.unwrap().is_empty());
}

#[tokio::test]
async fn cas_rechecks_project_head_without_repeating_preparation() {
    let target = tempfile::tempdir().unwrap();
    let store = Probe::new(vec![]);
    let at_conflict = Arc::new(Mutex::new(None));
    let observed = at_conflict.clone();
    let path = target.path().to_owned();
    *store.conflict_once.lock().unwrap() = Some(Box::new(move || {
        let prepared_head = git(&path, &["rev-parse", "HEAD"]);
        git(
            &path,
            &[
                "-c",
                "user.name=Fault",
                "-c",
                "user.email=fault@localhost",
                "commit",
                "--allow-empty",
                "--no-gpg-sign",
                "--no-verify",
                "-m",
                "advance during CAS",
            ],
        );
        assert_ne!(git(&path, &["rev-parse", "HEAD"]), prepared_head);
        *observed.lock().unwrap() = Some(snapshot(&path));
    }));
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    );
    let response = service.execute(command(target.path())).await;
    assert!(!response.ok, "stale preparation must not commit");
    let error = response.error.unwrap();
    assert_eq!(error.code, ErrorCode::RunQueueConflict);
    assert!(error.retryable, "{error:?}");
    assert_eq!(store.apply_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        snapshot(target.path()),
        at_conflict.lock().unwrap().clone().unwrap()
    );
    assert_no_records_or_events(&store).await;
}

#[tokio::test]
async fn cas_does_not_reinitialize_a_disappeared_git_root() {
    let target = tempfile::tempdir().unwrap();
    let store = Probe::new(vec![]);
    let path = target.path().to_owned();
    *store.conflict_once.lock().unwrap() = Some(Box::new(move || {
        std::fs::rename(path.join(".git"), path.join("retained-git")).unwrap();
    }));
    let response = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    )
    .execute(command(target.path()))
    .await;
    let error = response.error.unwrap();
    assert_eq!(error.code, ErrorCode::RunQueueConflict);
    assert!(error.retryable);
    assert!(!target.path().join(".git").exists());
    assert!(target.path().join("retained-git").is_dir());
    assert_eq!(store.apply_attempts.load(Ordering::SeqCst), 1);
    assert_no_records_or_events(&store).await;
}

#[tokio::test]
async fn derive_rejects_a_default_agent_change_after_worktree_preparation() {
    let target = tempfile::tempdir().unwrap();
    let store = Probe::new(vec![]);
    let service = crate::native_fixture::native_service(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
        Arc::new(NoNativeInput),
    );
    let CommandResult::Project(project) = service
        .execute(Command::RegisterProject {
            id: "p".into(),
            name: "Project".into(),
            workdir: Some(target.path().display().to_string()),
            repo_url: None,
        })
        .await
        .result
        .unwrap()
    else {
        panic!("Project")
    };
    for id in ["a", "b"] {
        let response = service
            .execute(Command::RegisterAgent {
                id: id.into(),
                name: format!("Agent {id}"),
                config: ait_contracts::AgentConfiguration {
                    provider_id: "builtin-codex".into(),
                    model: "gpt-5.6-sol".into(),
                    reasoning_effort: None,
                    system_prompt: None,
                },
            })
            .await;
        assert!(response.ok, "{:?}", response.error);
    }
    let mut settings = ait_contracts::default_settings();
    settings.0.insert("agents.default_agent".into(), json!("a"));
    let response = service
        .execute(Command::SaveSettings {
            expected_revision: 1,
            values: settings,
        })
        .await;
    assert!(response.ok, "{:?}", response.error);
    let response = service
        .execute(Command::CreateSession {
            id: "source".into(),
            project_id: project.id.clone(),
            agent_id: "a".into(),
            at_message_id: None,
        })
        .await;
    assert!(response.ok, "{:?}", response.error);

    let concurrent = store.clone();
    *store.conflict_once.lock().unwrap() = Some(Box::new(move || {
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let current = concurrent
                        .inner
                        .read(&[ControlFilter::id(Kind::Settings, "settings")])
                        .await
                        .unwrap();
                    let mut settings = current.records.into_iter().next().unwrap();
                    settings.value["values"]["agents.default_agent"] = json!("b");
                    settings.value["revision"] =
                        json!(settings.value["revision"].as_u64().unwrap() + 1);
                    concurrent
                        .inner
                        .apply(current.revision, vec![ControlChange::Put(settings)], vec![])
                        .await
                        .unwrap();
                });
        })
        .join()
        .unwrap();
    }));
    let attempts = store.apply_attempts.load(Ordering::SeqCst);
    let response = service
        .execute(Command::DeriveSession {
            id: "fork".into(),
            project_id: project.id,
            source_session_id: "source".into(),
            agent_id: String::new(),
            at_message_id: project.root_message_id,
            text: "continue".into(),
        })
        .await;
    let error = response.error.unwrap();
    assert_eq!(error.code, ErrorCode::RunQueueConflict);
    assert!(error.retryable, "{error:?}");
    assert_eq!(store.apply_attempts.load(Ordering::SeqCst), attempts + 1);
    assert!(!target.path().join(".ait/fork").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn cas_rechecks_canonical_target_even_when_head_is_unchanged() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let relocated = parent.path().join("relocated");
    std::fs::create_dir(&target).unwrap();
    let store = Probe::new(vec![]);
    let path = target.clone();
    let moved = relocated.clone();
    *store.conflict_once.lock().unwrap() = Some(Box::new(move || {
        let head = git(&path, &["rev-parse", "HEAD"]);
        std::fs::rename(&path, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &path).unwrap();
        assert_eq!(git(&path, &["rev-parse", "HEAD"]), head);
    }));
    let response = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    )
    .execute(command(&target))
    .await;
    let error = response.error.unwrap();
    assert_eq!(error.code, ErrorCode::RunQueueConflict);
    assert!(error.retryable);
    assert_eq!(
        target.canonicalize().unwrap(),
        relocated.canonicalize().unwrap()
    );
    assert_eq!(store.apply_attempts.load(Ordering::SeqCst), 1);
    assert_no_records_or_events(&store).await;
}

struct NoNativeInput;
#[async_trait::async_trait]
impl crate::native_fixture::NativeHandler for NoNativeInput {
    async fn invoke(
        &self,
        _: ait_ports::CodexThreadInvocation,
    ) -> Result<crate::native_fixture::NativeReply, ait_domain::DomainError> {
        panic!("changed default must reject before input");
    }
}
