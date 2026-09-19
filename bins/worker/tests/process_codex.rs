//! Native create/resume and history publication traverse the real worker process.
#![cfg(unix)]
#![allow(clippy::pedantic)]
use ait_domain::{ApprovalMode, RunPermissionProfile, SandboxAccess};
use ait_ports::{
    CodexThreadInvocation, CodexThreadWriter, DenyWorkspaceApprovals, WorkspaceProgressEvent,
    WorkspaceProgressReporter,
};
use async_trait::async_trait;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};
struct Progress;
#[async_trait]
impl WorkspaceProgressReporter for Progress {
    async fn report(&self, _event: WorkspaceProgressEvent) {}
}
fn fixture(
    scenario: &str,
) -> (
    tempfile::TempDir,
    ait_ipc::supervisor::WorkerSupervisor,
    CodexThreadInvocation,
) {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("codex-fixture.py");
    let source = include_str!("../../../crates/agent-adapters/src/codex/native/tests/fixture.py");
    let injected = format!(
        "#!/usr/bin/env python3\nimport sys\nsys.argv.extend([{}, {}])\n{}",
        serde_json::to_string(scenario).unwrap(),
        serde_json::to_string(&cwd.join("requests.jsonl")).unwrap(),
        source
    );
    std::fs::write(&binary, injected).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let worker =
        ait_ipc::supervisor::WorkerSupervisor::new(PathBuf::from(env!("CARGO_BIN_EXE_ait-worker")))
            .with_codex_binary(binary);
    let request = CodexThreadInvocation {
        project_execution: None,
        request_id: "run-input".into(),
        thread_id: None,
        developer_instructions: Some("Project instructions".into()),
        prompt: "new input only".into(),
        cwd,
        model: "test-model".into(),
        reasoning_effort: Some("high".into()),
        permission_profile: RunPermissionProfile {
            sandbox: SandboxAccess::ReadOnly,
            approval: ApprovalMode::OnRequest,
        },
        approvals: Arc::new(DenyWorkspaceApprovals),
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    (directory, worker, request)
}
#[tokio::test]
async fn prepared_worker_does_not_send_until_admitted_and_publishes_full_native_history() {
    let (_directory, worker, request) = fixture("complete");
    let log = request.cwd.join("requests.jsonl");
    let mut connection = worker.open(request).await.unwrap();
    assert!(connection.prepared().history.writer_confirmed);
    assert_eq!(connection.prepared().history.id, "thread");
    let before = std::fs::read_to_string(&log).unwrap();
    assert!(before.contains("thread/start"));
    assert!(!before.contains("turn/start"));
    assert!(!before.contains("thread/turns/list"));
    let history = connection.start(Arc::new(Progress)).await.unwrap();
    assert!(history.writer_confirmed);
    assert_eq!(history.turns[0].items[0]["clientId"], "run-input");
    assert_eq!(history.turns[0].items[2]["text"], "authoritative answer");
    connection.close().await;
    let log = std::fs::read_to_string(log).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("turn/start"))
            .count(),
        1
    );
    assert!(log.contains("new input only"));
    assert!(!log.contains("Conversation:"));
}
#[tokio::test]
async fn resume_uses_the_same_worker_protocol_without_developer_or_cwd_overrides() {
    let (_directory, worker, mut request) = fixture("complete");
    request.thread_id = Some("thread".into());
    request.developer_instructions = None;
    let mut connection = worker.open(request).await.unwrap();
    assert!(connection.prepared().history.writer_confirmed);
    assert!(connection.read().await.unwrap().turns.is_empty());
    connection.start(Arc::new(Progress)).await.unwrap();
    connection.close().await;
}
#[tokio::test]
async fn closing_a_prepared_worker_never_sends_and_reaps_its_app_server() {
    let (_directory, worker, request) = fixture("complete");
    let log = request.cwd.join("requests.jsonl");
    let mut connection = worker.open(request).await.unwrap();
    connection.close().await;
    assert!(
        !std::fs::read_to_string(&log)
            .unwrap()
            .contains("turn/start")
    );
    let pid = std::fs::read_to_string(log.with_extension("jsonl.pid")).unwrap();
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[tokio::test]
async fn history_catalog_and_title_all_use_reaped_auxiliary_worker_processes() {
    use ait_ports::{
        CodexHistorySource, HostProviderModelCatalog, SessionTitleGenerator, SessionTitleRequest,
    };
    let (directory, worker, request) = fixture("complete");
    let log = directory.path().join("aux.jsonl");
    let binary = directory.path().join("aux.py");
    std::fs::write(
        &binary,
        format!(
            "#!/usr/bin/env python3\nLOG={}\n{}",
            serde_json::to_string(&log).unwrap(),
            include_str!("fixtures/codex_aux.py")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let worker = worker.with_codex_binary(binary);
    let provider = ait_domain::AgentProvider {
        id: "codex".into(),
        name: "Codex".into(),
        kind: ait_domain::ProviderKind::Codex,
        url: None,
        models: vec![],
    };
    assert_eq!(
        worker
            .list_threads(&[ait_ports::CodexThreadSourceKind::AppServer])
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        worker.read_thread("aux-thread").await.unwrap().id,
        "aux-thread"
    );
    assert_eq!(
        worker.discover_models(&provider).await.unwrap()[0].id,
        "fixture-model"
    );
    let title = worker
        .generate(SessionTitleRequest {
            request_id: "metadata-only".into(),
            user_prompt: "Unify execution".into(),
            config: ait_domain::AgentConfiguration {
                provider_id: provider.id.clone(),
                model: "fixture-model".into(),
                reasoning_effort: None,
                system_prompt: None,
            },
            provider,
            credential_ref: None,
            cwd: request.cwd,
            cancellation: request.cancellation,
        })
        .await
        .unwrap();
    assert_eq!(title.title, "Review native execution");
    let events = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let pids = events
        .iter()
        .map(|event| event["pid"].as_u64().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(pids.len(), 4);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["request"]["method"] == "turn/start")
            .count(),
        1
    );
    for pid in pids {
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

#[tokio::test]
async fn native_item_budget_interrupts_the_turn_and_reports_a_limit() {
    let (_directory, worker, request) = fixture("budget");
    let log = request.cwd.join("requests.jsonl");
    let mut connection = worker.open(request).await.unwrap();
    let failure = connection.start(Arc::new(Progress)).await.unwrap_err();
    assert_eq!(failure.code, ait_domain::ErrorCode::RunLimitExceeded);
    connection.close().await;
    let calls = std::fs::read_to_string(log).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("turn/start"))
            .count(),
        1
    );
    assert!(calls.contains("turn/interrupt"));
}
