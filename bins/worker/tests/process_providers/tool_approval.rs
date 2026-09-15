//! Real worker, provider adapters and temporary SQLite; no model or private files.
use super::*;
use ait_contracts::ToolApprovalAction as Action;
use ait_domain::{SandboxAccess, ToolApprovalState as Status};
use std::time::Duration;

#[derive(Default)]
struct ObserveWorker(std::sync::atomic::AtomicU32);
impl ait_ipc::supervisor::WorkerObserver for ObserveWorker {
    fn checkpoint(
        &self,
        pid: u32,
        method: &str,
        _: ait_ipc::supervisor::CommitBoundary,
    ) -> Result<(), ait_contracts::worker::ProtocolError> {
        if method == "tool_approval" {
            self.0.store(pid, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

#[tokio::test]
async fn tool_approval_worker_loss_expires_waiter_and_never_replays_authority() {
    let kind = ProviderKind::OpenAI;
    let mut f = Fixture::new(
        kind,
        vec![
            response(
                kind,
                &[(
                    "one",
                    "write",
                    json!({"file_path":"effect.txt","content":"must not run"}),
                )],
            ),
            response(kind, &[]),
        ],
        "read_only",
    )
    .await;
    interactive(&mut f);
    let observer = Arc::new(ObserveWorker::default());
    f.service = f.service.clone().with_run_dispatcher(Arc::new(
        ait_ipc::supervisor::WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
            .with_observer(observer.clone()),
    ));
    let task = execute(&f);
    let run = pending(&f).await;
    let pid = observer.0.load(std::sync::atomic::Ordering::SeqCst);
    assert!(pid > 0);
    // Exact PID received from our own supervisor, never a process-name kill.
    assert!(
        std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .unwrap()
            .unwrap()
            .ok
    );
    let recovered = support::persisted_run(f.store.as_ref(), &run.id).await;
    assert_eq!(recovered.status, "completed");
    assert_eq!(recovered.tool_approvals[0].status, Status::Expired);
    assert!(recovered.lease_epoch > run.lease_epoch);
    assert!(!f.workdir.join("effect.txt").exists());
    assert_eq!(recovered.execution.as_ref().unwrap().tools.len(), 1);
    assert!(
        recovered.execution.as_ref().unwrap().tools[0]
            .tool_result_message_id
            .is_some()
    );
    assert!(
        f.service
            .resolve_tool_approval(
                &run.id,
                &run.tool_approvals[0].grant.request_id,
                Action::Approve
            )
            .await
            .is_err()
    );
    f.finish().await;
}

async fn pending(f: &Fixture) -> ait_contracts::RunView {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let runs = support::workspace(&f.service).await.runs;
            if let Some(run) = runs
                .into_iter()
                .find(|r| r.tool_approvals.iter().any(|a| a.status == Status::Pending))
            {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker must publish a durable reviewable request")
}
fn execute(f: &Fixture) -> tokio::task::JoinHandle<ait_contracts::Response> {
    let service = f.service.clone();
    tokio::spawn(async move {
        service
            .execute(Command::SendMessage {
                session_id: "session".into(),
                text: "Approval fixture".into(),
            })
            .await
    })
}
fn interactive(f: &mut Fixture) {
    f.service = f
        .service
        .clone()
        .with_api_tools(Arc::new(ait_tools::host::HostToolFactory));
}

#[tokio::test]
async fn tool_approval_sensitive_reasons_never_reach_cards_events_results_or_storage() {
    let marker = "NEC290_PRIVATE_APPROVAL_FIXTURE";
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        let reply = response(
            kind,
            &[(
                "one",
                "write",
                json!({
                    "file_path":"effect.txt", "content":"synthetic", "sandbox_permissions":"workspace-write",
                    "justification":format!("Authorization: Bearer {marker}"),
                }),
            )],
        );
        let mut f = Fixture::new(kind, vec![reply; 3], "read_only").await;
        interactive(&mut f);
        let run = f.run().await;
        assert!(matches!(run.status.as_str(), "failed" | "interrupted"));
        assert!(run.tool_approvals.is_empty());
        assert!(!f.workdir.join("effect.txt").exists());
        let archive = ok(
            &f.service,
            Command::ExportProject {
                project_id: "p".into(),
            },
        )
        .await;
        let mut surfaces = vec![
            serde_json::to_vec(&run).unwrap(),
            format!("{:?}", support::workspace(&f.service).await).into_bytes(),
            serde_json::to_vec(&archive).unwrap(),
            serde_json::to_vec(&f.service.replay_events(0, 1000).await.unwrap()).unwrap(),
        ];
        for path in [
            f.directory.path().join("ait.db"),
            f.directory.path().join("ait.db-wal"),
            f.project.path().join(".ait/project.sqlite3"),
            f.project.path().join(".ait/project.sqlite3-wal"),
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                surfaces.push(bytes);
            }
        }
        for bytes in surfaces {
            assert!(
                !bytes
                    .windows(marker.len())
                    .any(|window| window == marker.as_bytes())
            );
        }
        f.finish().await;
    }
}

#[tokio::test]
async fn tool_approval_consumed_grants_and_unknown_effects_are_never_replayed() {
    use ait_ipc::supervisor::CommitBoundary::{BeforeAck, BeforeCommit};
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        for (method, boundary, effect) in [
            ("tool_grant", BeforeAck, false),
            ("tool_outcome", BeforeCommit, true),
        ] {
            let mut f = Fixture::new(
                kind,
                vec![
                    response(
                        kind,
                        &[(
                            "one",
                            "write",
                            json!({"file_path":"effect.txt","content":"one effect"}),
                        )],
                    ),
                    response(kind, &[]),
                ],
                "read_only",
            )
            .await;
            interactive(&mut f);
            let fault = Arc::new(KillOnce {
                method,
                boundary,
                fired: std::sync::atomic::AtomicBool::new(false),
            });
            f.service = f.service.clone().with_run_dispatcher(Arc::new(
                ait_ipc::supervisor::WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
                    .with_observer(fault.clone()),
            ));
            let task = execute(&f);
            let run = pending(&f).await;
            f.service
                .resolve_tool_approval(
                    &run.id,
                    &run.tool_approvals[0].grant.request_id,
                    Action::Approve,
                )
                .await
                .unwrap();
            let _ = tokio::time::timeout(Duration::from_secs(20), task)
                .await
                .unwrap()
                .unwrap();
            let settled = support::persisted_run(f.store.as_ref(), &run.id).await;
            assert!(fault.fired.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(
                settled.status, "failed",
                "An unacknowledged effect must require manual inspection"
            );
            assert_eq!(settled.tool_approvals[0].status, Status::Consumed);
            assert_eq!(settled.execution.as_ref().unwrap().tools.len(), 1);
            assert!(
                settled.execution.as_ref().unwrap().tools[0]
                    .tool_result_message_id
                    .is_some()
            );
            assert_eq!(f.workdir.join("effect.txt").exists(), effect);
            if effect {
                assert_eq!(
                    std::fs::read_to_string(f.workdir.join("effect.txt")).unwrap(),
                    "one effect"
                );
            }
            assert!(
                f.service
                    .resolve_tool_approval(
                        &run.id,
                        &run.tool_approvals[0].grant.request_id,
                        Action::Approve
                    )
                    .await
                    .is_err()
            );
            f.finish().await;
        }
    }
}

#[tokio::test]
async fn tool_approval_openai_deepseek_approve_deny_cancel_expire_and_one_use() {
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        for decision in ["approve", "deny", "cancel", "expire"] {
            let args = json!({"file_path":"approved.txt","content":"synthetic content","sandbox_permissions":"workspace-write","justification":"Create the requested synthetic file"});
            let mut f = Fixture::new(
                kind,
                vec![
                    response(kind, &[("first", "write", args.clone())]),
                    response(kind, &[]),
                ],
                "read_only",
            )
            .await;
            interactive(&mut f);
            if decision == "expire" {
                f.service = f
                    .service
                    .clone()
                    .with_tool_approval_timeout(Duration::from_millis(300));
            }
            let task = execute(&f);
            let waiting = pending(&f).await;
            assert!(!f.workdir.join("approved.txt").exists());
            assert!(waiting.native_approvals.is_empty());
            let grant = &waiting.tool_approvals[0].grant;
            assert_eq!(grant.target.current, SandboxAccess::ReadOnly);
            assert_eq!(grant.target.requested, SandboxAccess::WorkspaceWrite);
            assert!(grant.target.operation.ends_with("approved.txt"));
            assert_eq!(grant.target.cwd, f.workdir.to_string_lossy());
            let stored = support::persisted_run(f.store.as_ref(), &waiting.id).await;
            assert_eq!(stored.tool_approvals, waiting.tool_approvals);
            // Queries and events remain available while the private RPC waits.
            assert!(
                f.service
                    .replay_events(0, 1000)
                    .await
                    .unwrap()
                    .iter()
                    .any(|e| e.kind == "run.tool_approval_requested")
            );
            if decision != "expire" {
                let action = match decision {
                    "approve" => Action::Approve,
                    "deny" => Action::Deny,
                    _ => Action::Cancel,
                };
                f.service
                    .resolve_tool_approval(&waiting.id, &grant.request_id, action)
                    .await
                    .unwrap();
                assert!(
                    f.service
                        .resolve_tool_approval(&waiting.id, &grant.request_id, Action::Approve)
                        .await
                        .is_err()
                );
            }
            let result = tokio::time::timeout(Duration::from_secs(20), task)
                .await
                .unwrap()
                .unwrap();
            assert!(result.ok, "{result:?}");
            let run = support::persisted_run(f.store.as_ref(), &waiting.id).await;
            assert_eq!(run.permission_profile.sandbox, SandboxAccess::ReadOnly);
            assert_eq!(
                run.status,
                if decision == "cancel" {
                    "cancelled"
                } else {
                    "completed"
                }
            );
            assert_eq!(
                run.tool_approvals[0].status,
                match decision {
                    "approve" => Status::Consumed,
                    "deny" => Status::Denied,
                    "cancel" => Status::Cancelled,
                    _ => Status::Expired,
                }
            );
            assert_eq!(
                f.workdir.join("approved.txt").exists(),
                decision == "approve"
            );
            let tools = &run.execution.as_ref().unwrap().tools;
            assert_eq!(tools.len(), 1);
            assert!(tools[0].tool_result_message_id.is_some());
            assert_eq!(
                tools[0].status,
                match decision {
                    "approve" => ait_domain::ToolExecutionStatus::Succeeded,
                    "cancel" => ait_domain::ToolExecutionStatus::Cancelled,
                    _ => ait_domain::ToolExecutionStatus::Denied,
                }
            );
            assert_eq!(
                f.requests.lock().unwrap().len(),
                if decision == "cancel" { 1 } else { 2 }
            );
            assert!(
                f.service
                    .resolve_tool_approval(&waiting.id, &grant.request_id, Action::Approve)
                    .await
                    .is_err()
            );
            f.finish().await;
        }
    }
}

#[tokio::test]
async fn tool_approval_each_call_requires_new_grant_and_run_settings_do_not_change() {
    let kind = ProviderKind::DeepSeek;
    let mut f = Fixture::new(
        kind,
        vec![
            response(
                kind,
                &[
                    (
                        "one",
                        "write",
                        json!({"file_path":"one.txt","content":"one"}),
                    ),
                    (
                        "two",
                        "write",
                        json!({"file_path":"two.txt","content":"two"}),
                    ),
                ],
            ),
            response(kind, &[]),
        ],
        "read_only",
    )
    .await;
    interactive(&mut f);
    let before = ok(&f.service, Command::GetSettings).await;
    let task = execute(&f);
    let one = pending(&f).await;
    let first = &one.tool_approvals[0].grant;
    f.service
        .resolve_tool_approval(&one.id, &first.request_id, Action::Approve)
        .await
        .unwrap();
    let two = pending(&f).await;
    let second = two
        .tool_approvals
        .iter()
        .find(|a| a.status == Status::Pending)
        .unwrap();
    assert_ne!(first.request_id, second.grant.request_id);
    assert_ne!(first.execution_id, second.grant.execution_id);
    assert_eq!(
        std::fs::read_to_string(f.workdir.join("one.txt")).unwrap(),
        "one"
    );
    assert!(!f.workdir.join("two.txt").exists());
    f.service
        .resolve_tool_approval(&two.id, &second.grant.request_id, Action::Deny)
        .await
        .unwrap();
    assert!(task.await.unwrap().ok);
    assert_eq!(before, ok(&f.service, Command::GetSettings).await);
    assert!(!f.workdir.join("two.txt").exists());
    f.finish().await;
}

#[tokio::test]
async fn tool_approval_path_or_administrator_change_revokes_pending_authority() {
    for change in ["symlink", "administrator"] {
        let kind = ProviderKind::OpenAI;
        let mut f = Fixture::new(
            kind,
            vec![
                response(
                    kind,
                    &[(
                        "one",
                        "write",
                        json!({"file_path":"approved.txt","content":"one"}),
                    )],
                ),
                response(kind, &[]),
            ],
            "read_only",
        )
        .await;
        interactive(&mut f);
        let task = execute(&f);
        let run = pending(&f).await;
        let approval = &run.tool_approvals[0];
        let outside = tempfile::tempdir().unwrap();
        let synthetic = outside.path().join("synthetic.txt");
        std::fs::write(&synthetic, "unchanged").unwrap();
        let resolver = if change == "symlink" {
            std::os::unix::fs::symlink(&synthetic, f.workdir.join("approved.txt")).unwrap();
            f.service.clone()
        } else {
            f.service
                .clone()
                .with_permission_limits(ait_application::PermissionPolicyLimits {
                    max_sandbox: SandboxAccess::ReadOnly,
                    allow_session_approvals: true,
                })
        };
        assert!(
            resolver
                .resolve_tool_approval(&run.id, &approval.grant.request_id, Action::Approve)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(15), task)
                .await
                .unwrap()
                .unwrap()
                .ok
        );
        assert_eq!(std::fs::read_to_string(synthetic).unwrap(), "unchanged");
        assert_eq!(
            support::persisted_run(f.store.as_ref(), &run.id)
                .await
                .tool_approvals[0]
                .status,
            Status::Expired
        );
        f.finish().await;
    }
}

#[tokio::test]
async fn tool_approval_ceiling_and_unreviewable_targets_never_create_pending() {
    for args in [
        json!({"file_path":"allowed.txt","content":"one","sandbox_permissions":"danger-full-access"}),
        json!({"file_path":"../escape","content":"one"}),
    ] {
        let kind = ProviderKind::OpenAI;
        let mut f = Fixture::new(
            kind,
            vec![
                response(kind, &[("one", "write", args)]),
                response(kind, &[]),
            ],
            "read_only",
        )
        .await;
        interactive(&mut f);
        f.service =
            f.service
                .clone()
                .with_permission_limits(ait_application::PermissionPolicyLimits {
                    max_sandbox: SandboxAccess::WorkspaceWrite,
                    allow_session_approvals: true,
                });
        let run = f.run().await;
        assert_eq!(run.status, "completed");
        assert!(run.tool_approvals.is_empty());
        assert_eq!(
            run.execution.unwrap().tools[0].status,
            ait_domain::ToolExecutionStatus::Denied
        );
        assert!(!f.workdir.join("allowed.txt").exists());
        f.finish().await;
    }
}

#[tokio::test]
async fn tool_approval_shell_uses_effective_os_sandbox_for_exactly_one_command() {
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        let mut f = Fixture::new(kind, vec![response(kind, &[("shell-one", "bash", json!({
            "command":"printf approved > shell.txt", "description":"Write a synthetic file", "sandbox_permissions":"workspace-write", "justification":"Write a synthetic file"
        }))]), response(kind, &[("shell-two", "bash", json!({"command":"printf unexpected > forbidden.txt", "description":"Verify readonly baseline"}))]), response(kind, &[])], "read_only").await;
        interactive(&mut f);
        let task = execute(&f);
        let run = pending(&f).await;
        assert!(!f.workdir.join("shell.txt").exists());
        f.service
            .resolve_tool_approval(
                &run.id,
                &run.tool_approvals[0].grant.request_id,
                Action::Approve,
            )
            .await
            .unwrap();
        assert!(task.await.unwrap().ok);
        assert_eq!(
            std::fs::read_to_string(f.workdir.join("shell.txt")).unwrap(),
            "approved"
        );
        assert!(
            !f.workdir.join("forbidden.txt").exists(),
            "next independent command must retain readonly OS policy"
        );
        let run = support::persisted_run(f.store.as_ref(), &run.id).await;
        assert_eq!(run.tool_approvals.len(), 1);
        assert_eq!(run.permission_profile.sandbox, SandboxAccess::ReadOnly);
        f.finish().await;
    }
}
