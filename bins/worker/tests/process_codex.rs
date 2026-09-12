//! Real Codex process failures preserve the existing Git settlement journal.
#![cfg(unix)]
#![allow(clippy::pedantic)]
#[path = "../../../crates/application/tests/support.rs"]
mod support;
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, default_settings};
use ait_ipc::supervisor::{CommitBoundary, WorkerSupervisor};
use ait_storage_sqlite::SqliteControlStore;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};
async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let r = service.execute(command).await;
    assert!(r.ok, "{:?}", r.error);
    r.result.unwrap()
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn install_codex(path: &Path) {
    std::fs::write(path, r#"#!/bin/sh
[ "$1" = "app-server" ] || exit 2
printf '%s\n' invoked >> "$(dirname "$0")/invocations"
IFS= read -r line || exit 3
printf '%s\n' '{"id":0,"result":{}}'
IFS= read -r line || exit 3
IFS= read -r line || exit 3
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-fault"}}}'
IFS= read -r line || exit 3
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-fault"}}}'
printf '%s\n' 'exactly once' > native-effect.txt
printf '%s\n' '{"method":"item/completed","params":{"threadId":"thread-fault","turnId":"turn-fault","item":{"type":"commandExecution","id":"native-command","status":"completed","command":"pwd","aggregatedOutput":"project"}}}'
printf '%s\n' '{"method":"item/completed","params":{"threadId":"thread-fault","turnId":"turn-fault","item":{"type":"agentMessage","id":"final","phase":"final_answer","text":"Native change saved"}}}'
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-fault","turn":{"id":"turn-fault","items":[],"status":"completed"}}}'
"#).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(unix)]
struct KillOnce {
    method: &'static str,
    boundary: ait_ipc::supervisor::CommitBoundary,
    fired: std::sync::atomic::AtomicBool,
}
#[cfg(unix)]
impl ait_ipc::supervisor::WorkerObserver for KillOnce {
    fn checkpoint(
        &self,
        pid: u32,
        method: &str,
        boundary: ait_ipc::supervisor::CommitBoundary,
    ) -> Result<(), ait_contracts::worker::ProtocolError> {
        if method == self.method
            && boundary == self.boundary
            && !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            assert!(
                std::process::Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
            return Err(ait_contracts::worker::ProtocolError::UnexpectedEof);
        }
        Ok(())
    }
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_checkpoint_ack_kill_matrix_preserves_message_and_git_commit() {
    use CommitBoundary::{AfterAck, BeforeAck, BeforeCommit};
    for method in [
        "workspace_checkpoint",
        "workspace_integration",
        "workspace_finished",
        "cost_ceiling",
    ] {
        for boundary in [BeforeCommit, BeforeAck, AfterAck] {
            if method == "cost_ceiling" && boundary != BeforeCommit {
                continue;
            }
            let root = tempfile::tempdir().unwrap();
            let project = root.path().join("project");
            std::fs::create_dir(&project).unwrap();
            let binary = root.path().join("codex");
            install_codex(&binary);
            let fault = Arc::new(KillOnce {
                method,
                boundary,
                fired: std::sync::atomic::AtomicBool::new(false),
            });
            let supervisor = Arc::new(
                WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
                    .with_codex_binary(binary)
                    .with_cost_ceiling((method == "cost_ceiling").then_some(10_000_000))
                    .with_observer(fault.clone()),
            );
            let store = Arc::new(SqliteControlStore::open(root.path().join("ait.db")).unwrap());
            let service = LocalControlService::with_workspace_agent(store, supervisor.clone())
                .with_run_dispatcher(supervisor);
            let mut settings = default_settings();
            settings
                .0
                .insert("permissions.sandbox".into(), json!("workspace_write"));
            for command in [
                Command::SaveSettings {
                    expected_revision: 1,
                    values: settings,
                },
                Command::RegisterProject {
                    id: "p".into(),
                    name: "Project".into(),
                    workdir: Some(project.display().to_string()),
                    repo_url: None,
                },
                Command::RegisterAgent {
                    id: "a".into(),
                    name: "Codex".into(),
                    config: ait_contracts::AgentConfiguration {
                        provider_id: "builtin-codex".into(),
                        model: "gpt-5.6-sol".into(),
                        reasoning_effort: None,
                    },
                },
                Command::CreateSession {
                    id: "s".into(),
                    project_id: "p".into(),
                    agent_id: "a".into(),
                    at_message_id: None,
                },
            ] {
                ok(&service, command).await;
            }
            let baseline = git(&project, &["rev-parse", "HEAD"]);
            let CommandResult::Run(run) = ok(
                &service,
                Command::SendMessage {
                    session_id: "s".into(),
                    text: "Make the native change".into(),
                },
            )
            .await
            else {
                panic!()
            };
            if method == "cost_ceiling" {
                assert_eq!(run.status, "failed");
                assert!(
                    !root.path().join("invocations").exists(),
                    "unpriced native Provider was started"
                );
                assert!(!project.join("native-effect.txt").exists());
                let view = support::workspace(&service).await;
                assert!(view.sessions[0].active_run_id.is_none());
                assert!(
                    view.messages
                        .iter()
                        .all(|message| message.role != "assistant")
                );
                continue;
            }
            assert!(
                fault.fired.load(std::sync::atomic::Ordering::SeqCst),
                "{method} {boundary:?}: {run:?}"
            );
            let view = support::workspace(&service).await;
            assert!(
                view.sessions[0].active_run_id.is_none(),
                "Session stuck: {run:?}"
            );
            assert_eq!(
                std::fs::read_to_string(root.path().join("invocations"))
                    .unwrap()
                    .lines()
                    .count(),
                1,
                "provider replayed"
            );
            assert_eq!(view.runs.len(), 1, "new Run allocated on recovery");
            let messages = view
                .messages
                .iter()
                .filter(|m| m.role == "assistant")
                .collect::<Vec<_>>();
            assert_eq!(
                git(
                    &project,
                    &["rev-list", "--all", "--count", &format!("{baseline}..")]
                ),
                "1",
                "duplicate Git commit"
            );
            if method == "workspace_checkpoint" && boundary == BeforeCommit {
                assert_eq!(
                    run.status, "failed",
                    "unacknowledged result must not be invented"
                );
                assert!(messages.is_empty());
            } else {
                assert_eq!(run.status, "completed", "{method} {boundary:?}: {run:?}");
                assert_eq!(messages.len(), 1);
                assert_eq!(
                    std::fs::read_to_string(project.join("native-effect.txt")).unwrap(),
                    "exactly once\n"
                );
                assert!(
                    run.execution.is_none(),
                    "native operations disguised as AIT ToolUse"
                );
                assert!(
                    !messages[0]
                        .data
                        .as_ref()
                        .unwrap()
                        .to_string()
                        .contains("tool_use")
                );
            }
            let reopened = LocalControlService::new(Arc::new(
                SqliteControlStore::open(root.path().join("ait.db")).unwrap(),
            ));
            assert_eq!(support::workspace(&reopened).await, view);
        }
    }
}
