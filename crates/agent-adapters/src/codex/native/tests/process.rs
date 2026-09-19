//! Real stdio process ownership with an offline app-server fixture.
#![allow(clippy::pedantic)]
use super::super::super::{CodexAppServerAdapter, CodexAppServerConfig};
use ait_domain::{ApprovalMode, ErrorCode, RunPermissionProfile, SandboxAccess};
use ait_ports::{
    CodexThreadInvocation, CodexThreadWriter, DenyWorkspaceApprovals, WorkspaceProgressEvent,
    WorkspaceProgressReporter,
};
use async_trait::async_trait;
use std::{os::unix::fs::PermissionsExt, sync::Arc};

struct Progress;
#[async_trait]
impl WorkspaceProgressReporter for Progress {
    async fn report(&self, _event: WorkspaceProgressEvent) {}
}

fn fixture(
    scenario: &str,
) -> (
    tempfile::TempDir,
    CodexAppServerAdapter,
    CodexThreadInvocation,
) {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("app-server-fixture.py");
    std::fs::write(&binary, include_str!("fixture.py")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = CodexAppServerAdapter::new(CodexAppServerConfig {
        codex_binary: binary,
        extra_args: vec![scenario.into(), cwd.join("requests.jsonl").into_os_string()],
        ..CodexAppServerConfig::default()
    })
    .unwrap();
    let request = CodexThreadInvocation {
        project_execution: None,
        request_id: "correlation".into(),
        thread_id: Some("thread".into()),
        developer_instructions: None,
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
    (directory, adapter, request)
}

#[tokio::test]
async fn resume_rejects_auto_reviewer_and_changed_cwd_before_any_input() {
    for scenario in ["reviewer", "cwd", "busy"] {
        let (_directory, adapter, request) = fixture(scenario);
        let log = request.cwd.join("requests.jsonl");
        let result = adapter.open(request).await;
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("must reject incompatible resume"),
        };
        assert_eq!(
            failure.code,
            if scenario == "busy" {
                ErrorCode::CodexThreadWriterBusy
            } else {
                ErrorCode::CodexThreadCapabilityUnsupported
            }
        );
        assert!(!std::fs::read_to_string(log).unwrap().contains("turn/start"));
    }
}

#[tokio::test]
async fn native_completion_rereads_full_items_and_pins_approval_policy() {
    for scenario in ["complete", "cancel"] {
        let (_directory, adapter, request) = fixture(scenario);
        let mut connection = adapter.open(request).await.unwrap();
        assert_eq!(
            connection.prepared().reasoning_effort.as_deref(),
            Some("high")
        );
        assert_eq!(connection.prepared().model_provider, "openai");
        assert!(connection.prepared().history.turns.is_empty());
        let snapshot = connection.start(Arc::new(Progress)).await.unwrap();
        assert!(snapshot.writer_confirmed);
        assert_eq!(snapshot.turns[0].items.len(), 3);
        assert_eq!(snapshot.turns[0].items[0]["clientId"], "correlation");
        assert_eq!(snapshot.turns[0].items[2]["text"], "authoritative answer");
        if scenario == "cancel" {
            assert_eq!(snapshot.turns[0].completed_at, None);
        }
        connection.close().await;
        connection.close().await;
    }
}

#[tokio::test]
async fn explicit_input_rejection_is_distinct_from_disconnection_after_send() {
    for (scenario, expected) in [
        ("reject", ErrorCode::CodexInputNotAccepted),
        ("disconnect", ErrorCode::CodexInputOutcomeUnknown),
    ] {
        let (_directory, adapter, request) = fixture(scenario);
        let mut connection = adapter.open(request).await.unwrap();
        let failure = connection.start(Arc::new(Progress)).await.unwrap_err();
        assert_eq!(failure.code, expected);
        connection.close().await;
    }
}

#[tokio::test]
async fn cancellation_confirms_interrupted_history_and_reaps_the_writer() {
    let (_directory, adapter, request) = fixture("wait_cancel");
    let token = request.cancellation.clone();
    let log = request.cwd.join("requests.jsonl");
    let mut connection = adapter.open(request).await.unwrap();
    let cancel = async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !std::fs::read_to_string(&log)
                .unwrap()
                .contains("turn/start")
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        token.cancel();
    };
    let (snapshot, ()) = tokio::join!(connection.start(Arc::new(Progress)), cancel);
    let snapshot = snapshot.unwrap();
    assert_eq!(snapshot.turns[0].status, "interrupted");
    assert_eq!(snapshot.turns[0].completed_at, None);
    assert!(snapshot.writer_confirmed);
    connection.close().await;
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
