//! Unified native admission and independent, repeatable Ait Git finalization.
#![allow(clippy::pedantic)]
mod fixtures;
mod support;
use ait_contracts::{Command, CommandResult, RunCommitStatus};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::CodexThreadInvocation;
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use fixtures::control_fixtures::{config, ok, send_text, setup, view};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::native::{NativeHandler, NativeReply};

#[derive(Default)]
struct Writer {
    requests: Mutex<Vec<CodexThreadInvocation>>,
    lock: Mutex<Option<PathBuf>>,
    fail_git_once: bool,
    reject_once: AtomicUsize,
    entered: Option<Arc<tokio::sync::Semaphore>>,
    release: Option<Arc<tokio::sync::Semaphore>>,
}
fn git(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
#[async_trait]
impl NativeHandler for Writer {
    async fn invoke(&self, request: CodexThreadInvocation) -> Result<NativeReply, DomainError> {
        self.requests.lock().unwrap().push(request.clone());
        if self.reject_once.fetch_add(1, Ordering::SeqCst) == 0 && request.prompt == "reject" {
            return Err(DomainError::invariant(
                ErrorCode::CodexInputNotAccepted,
                "input rejected",
            ));
        }
        if let Some(entered) = &self.entered {
            entered.add_permits(1);
        }
        if let Some(release) = &self.release {
            release.acquire().await.unwrap().forget();
        }
        if request.prompt != "no changes" {
            std::fs::write(request.cwd.join("result.txt"), &request.prompt).unwrap();
        }
        if request.prompt == "cancel" {
            request.cancellation.cancelled().await;
            return Err(DomainError::invariant(ErrorCode::RunCancelled, "cancelled"));
        }
        if request.prompt == "fail" {
            return Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "failed after writing",
            ));
        }
        if self.fail_git_once && self.requests.lock().unwrap().len() == 1 {
            let path = PathBuf::from(git(&request.cwd, &["rev-parse", "--absolute-git-dir"]))
                .join("HEAD.lock");
            std::fs::write(&path, "external lock").unwrap();
            *self.lock.lock().unwrap() = Some(path);
        }
        Ok(NativeReply {
            assistant_text: format!("done: {}", request.prompt),
            ..Default::default()
        })
    }
}
async fn enable(service: &ait_application::LocalControlService, enabled: bool) {
    let CommandResult::Settings(current) = ok(service, Command::GetSettings).await else {
        panic!()
    };
    let mut values = current.values;
    values
        .0
        .insert("codex.auto_commit".into(), serde_json::json!(enabled));
    ok(
        service,
        Command::SaveSettings {
            expected_revision: current.revision,
            values,
        },
    )
    .await;
}
fn service(writer: Arc<Writer>) -> ait_application::LocalControlService {
    support::native::native_service(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        writer,
    )
}
#[tokio::test]
async fn create_and_resume_send_only_new_input_in_one_fixed_session_directory() {
    let writer = Arc::new(Writer::default());
    let service = service(writer.clone());
    let directory = setup(&service, config("high")).await;
    ok(&service, send_text("one", "first input")).await;
    ok(&service, send_text("one", "second input")).await;
    let requests = writer.requests.lock().unwrap();
    assert!(requests[0].thread_id.is_none());
    assert!(requests[1].thread_id.is_some());
    assert_eq!(requests[0].cwd, requests[1].cwd);
    assert_eq!(
        requests[0].cwd,
        directory.path().canonicalize().unwrap().join(".ait/one")
    );
    assert_eq!(requests[1].prompt, "second input");
    assert!(requests[1].developer_instructions.is_none());
    assert_eq!(git(&requests[0].cwd, &["rev-list", "--count", "HEAD"]), "1");
}
#[tokio::test]
async fn auto_commit_is_a_run_receipt_and_retry_never_invokes_codex_again() {
    let writer = Arc::new(Writer {
        fail_git_once: true,
        ..Default::default()
    });
    let service = service(writer.clone());
    let _directory = setup(&service, config("high")).await;
    enable(&service, true).await;
    let CommandResult::Run(run) = ok(&service, send_text("one", "generated")).await else {
        panic!()
    };
    assert_eq!(run.status, "completed");
    assert!(run.error.is_none());
    let receipt = run.git_commit.unwrap();
    assert_eq!(receipt.status, RunCommitStatus::Failed);
    let planned = receipt.commit_id.unwrap();
    let before = view(&service).await.messages;
    std::fs::remove_file(writer.lock.lock().unwrap().take().unwrap()).unwrap();
    let CommandResult::Run(retried) =
        ok(&service, Command::RetryRunCommit { run_id: run.id }).await
    else {
        panic!()
    };
    let receipt = retried.git_commit.unwrap();
    assert_eq!(receipt.status, RunCommitStatus::Committed);
    assert_eq!(receipt.commit_id.as_deref(), Some(planned.as_str()));
    assert_eq!(writer.requests.lock().unwrap().len(), 1);
    assert_eq!(view(&service).await.messages, before);
    let cwd = writer.requests.lock().unwrap()[0].cwd.clone();
    assert_eq!(git(&cwd, &["rev-parse", "HEAD"]), planned);
    assert!(git(&cwd, &["status", "--porcelain"]).is_empty());
}
#[tokio::test]
async fn shutdown_rejects_git_retry_without_changing_the_failed_receipt() {
    let writer = Arc::new(Writer {
        fail_git_once: true,
        ..Default::default()
    });
    let service = service(writer.clone());
    let _directory = setup(&service, config("high")).await;
    enable(&service, true).await;
    let CommandResult::Run(run) = ok(&service, send_text("one", "generated")).await else {
        panic!()
    };
    assert_eq!(
        run.git_commit.as_ref().unwrap().status,
        RunCommitStatus::Failed
    );
    std::fs::remove_file(writer.lock.lock().unwrap().take().unwrap()).unwrap();
    service.begin_shutdown().await.unwrap();
    let response = service
        .execute(Command::RetryRunCommit {
            run_id: run.id.clone(),
        })
        .await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunCancelled);
    assert_eq!(view(&service).await.runs[0], run);
    assert_eq!(writer.requests.lock().unwrap().len(), 1);
    assert!(service.runs_drained());
}

#[tokio::test]
async fn dirty_baseline_disables_only_auto_commit_and_keeps_both_changes() {
    let writer = Arc::new(Writer::default());
    let service = service(writer.clone());
    let directory = setup(&service, config("high")).await;
    enable(&service, true).await;
    std::fs::write(directory.path().join(".ait/one/manual.txt"), "user change").unwrap();
    let CommandResult::Run(run) = ok(&service, send_text("one", "generated")).await else {
        panic!()
    };
    assert_eq!(run.status, "completed");
    assert_eq!(run.git_commit.unwrap().status, RunCommitStatus::Skipped);
    assert_eq!(
        std::fs::read_to_string(directory.path().join(".ait/one/manual.txt")).unwrap(),
        "user change"
    );
    assert_eq!(
        git(
            &writer.requests.lock().unwrap()[0].cwd,
            &["rev-list", "--count", "HEAD"]
        ),
        "1"
    );
}
#[tokio::test]
async fn rejected_first_input_releases_unmaterialized_thread_without_a_fake_user_message() {
    let writer = Arc::new(Writer::default());
    let service = service(writer.clone());
    let _directory = setup(&service, config("high")).await;
    let CommandResult::Run(run) = ok(&service, send_text("one", "reject")).await else {
        panic!()
    };
    assert_eq!(run.status, "failed");
    assert!(
        view(&service)
            .await
            .messages
            .iter()
            .all(|message| message.text.as_deref() != Some("reject"))
    );
    ok(&service, send_text("one", "accepted")).await;
    assert!(
        writer
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.thread_id.is_none())
    );
}

#[tokio::test]
async fn auto_commit_setting_is_frozen_before_model_execution() {
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let writer = Arc::new(Writer {
        entered: Some(entered.clone()),
        release: Some(release.clone()),
        ..Default::default()
    });
    let service = Arc::new(service(writer.clone()));
    let _directory = setup(&service, config("high")).await;
    enable(&service, true).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send_text("one", "frozen setting")).await })
    };
    entered.acquire().await.unwrap().forget();
    enable(&service, false).await;
    release.add_permits(1);
    let CommandResult::Run(run) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(run.git_commit.unwrap().status, RunCommitStatus::Committed);
    release.add_permits(1);
    let CommandResult::Run(next) = ok(&service, send_text("one", "later setting")).await else {
        panic!()
    };
    assert!(next.git_commit.is_none());
    assert_eq!(
        git(
            &writer.requests.lock().unwrap()[0].cwd,
            &["rev-list", "--count", "HEAD"]
        ),
        "2"
    );
}

#[tokio::test]
async fn unchanged_failed_and_cancelled_turns_never_create_commits() {
    for prompt in ["no changes", "fail", "cancel"] {
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let writer = Arc::new(Writer {
            entered: Some(entered.clone()),
            ..Default::default()
        });
        let service = Arc::new(service(writer.clone()));
        let directory = setup(&service, config("high")).await;
        enable(&service, true).await;
        let running = {
            let service = service.clone();
            tokio::spawn(async move { ok(&service, send_text("one", prompt)).await })
        };
        entered.acquire().await.unwrap().forget();
        if prompt == "cancel" {
            let active = view(&service).await.runs[0].id.clone();
            ok(&service, Command::CancelRun { run_id: active }).await;
        }
        let CommandResult::Run(run) = running.await.unwrap() else {
            panic!()
        };
        assert_eq!(run.git_commit.unwrap().status, RunCommitStatus::Skipped);
        assert_eq!(
            run.status,
            match prompt {
                "no changes" => "completed",
                "fail" => "failed",
                _ => "cancelled",
            }
        );
        let cwd = directory.path().join(".ait/one");
        assert_eq!(git(&cwd, &["rev-list", "--count", "HEAD"]), "1");
        if prompt != "no changes" {
            assert_eq!(
                std::fs::read_to_string(cwd.join("result.txt")).unwrap(),
                prompt
            );
        }
    }
}

#[tokio::test]
async fn startup_finishes_a_published_commit_receipt_without_opening_codex() {
    use support::ControlStoreTestExt;
    let writer = Arc::new(Writer::default());
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = support::native::native_service(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
        writer.clone(),
    );
    let _directory = setup(&service, config("high")).await;
    enable(&service, true).await;
    let CommandResult::Run(run) = ok(&service, send_text("one", "durable result")).await else {
        panic!()
    };
    let messages = view(&service).await.messages;
    let commit_id = run.git_commit.unwrap().commit_id.unwrap();
    let snapshot = store.load().await.unwrap();
    let mut state = snapshot.value;
    let record = &mut state["runs"][0];
    record["status"] = serde_json::json!("settling");
    record["phase"] = serde_json::json!("settling");
    record["auto_commit"]["view"]["status"] = serde_json::json!("prepared");
    state["sessions"][0]["active_run_id"] = serde_json::json!(run.id);
    store
        .commit(snapshot.revision, state, vec![])
        .await
        .unwrap();
    let restarted = ait_application::LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store,
    );
    let recovered = restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(
        recovered[0]
            .git_commit
            .as_ref()
            .unwrap()
            .commit_id
            .as_deref(),
        Some(commit_id.as_str())
    );
    assert_eq!(
        recovered[0].git_commit.as_ref().unwrap().status,
        RunCommitStatus::Committed
    );
    assert_eq!(view(&restarted).await.messages, messages);
    assert!(view(&restarted).await.sessions[0].active_run_id.is_none());
    assert_eq!(writer.requests.lock().unwrap().len(), 1);
    assert_eq!(
        git(
            &writer.requests.lock().unwrap()[0].cwd,
            &["rev-list", "--count", "HEAD"]
        ),
        "2"
    );
}

struct GatedOpening {
    inner: support::native::NativeFixture,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}
#[async_trait]
impl ait_ports::CodexThreadWriter for GatedOpening {
    async fn open(
        &self,
        request: CodexThreadInvocation,
    ) -> Result<Box<dyn ait_ports::CodexThreadConnection>, DomainError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        ait_ports::CodexThreadWriter::open(&self.inner, request).await
    }
}
#[tokio::test]
async fn shutdown_does_not_wait_for_native_writer_preparation_or_admit_late_input() {
    let handler = Arc::new(Writer::default());
    let writer = Arc::new(GatedOpening {
        inner: support::native::NativeFixture {
            handler: handler.clone(),
            histories: Arc::default(),
        },
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let service = Arc::new(
        ait_application::LocalControlService::new(
            Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
            Arc::new(SqliteControlStore::in_memory().unwrap()),
        )
        .with_codex_thread_writer(writer.clone()),
    );
    let _directory = setup(&service, config("high")).await;
    let admission = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send_text("one", "must not send")).await })
    };
    writer.entered.acquire().await.unwrap().forget();
    tokio::time::timeout(std::time::Duration::from_secs(2), service.begin_shutdown())
        .await
        .unwrap()
        .unwrap();
    writer.release.add_permits(1);
    let rejected = admission.await.unwrap();
    assert_eq!(rejected.error.unwrap().code, ErrorCode::RunCancelled);
    assert!(handler.requests.lock().unwrap().is_empty());
    assert!(view(&service).await.runs.is_empty());
}
