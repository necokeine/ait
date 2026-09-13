//! Native approvals regression coverage.
#![allow(clippy::pedantic)]

mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{
    config, ok, save_permission_settings, send, setup, view, wait_for_signal,
};
use crate::support::ControlStoreTestExt;
use ait_agent_adapters::codex::CodexWorkspaceAgent;
use ait_application::{LocalControlService, PermissionPolicyLimits};
use ait_contracts::{Command, NativeApprovalAction};
use ait_domain::{
    ApprovalGrantScope, DomainError, ErrorCode, NativeApprovalKind, NativeApprovalTarget,
    SandboxAccess,
};
use ait_ports::{
    WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceApprovalDecision,
    WorkspaceApprovalRequest,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

struct ApprovalAgent {
    kind: NativeApprovalKind,
    permission_path: Option<String>,
    target: Option<NativeApprovalTarget>,
    requested: Semaphore,
    decision: Mutex<Option<WorkspaceApprovalDecision>>,
}

impl ApprovalAgent {
    fn new(kind: NativeApprovalKind) -> Self {
        Self {
            kind,
            permission_path: None,
            target: None,
            requested: Semaphore::new(0),
            decision: Mutex::new(None),
        }
    }

    fn permissions_at(path: impl Into<String>) -> Self {
        Self {
            kind: NativeApprovalKind::Permissions,
            permission_path: Some(path.into()),
            target: None,
            requested: Semaphore::new(0),
            decision: Mutex::new(None),
        }
    }
}

#[async_trait]
impl WorkspaceAgent for ApprovalAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.requested.add_permits(1);
        let permission_path = self
            .permission_path
            .clone()
            .unwrap_or_else(|| request.cwd.to_string_lossy().into_owned());
        let decision = request
            .approvals
            .decide(WorkspaceApprovalRequest {
                run_id: request.request_id.clone(),
                protocol_request_id: serde_json::json!(73),
                method: match self.kind {
                    NativeApprovalKind::Permissions => "item/permissions/requestApproval",
                    NativeApprovalKind::FileChange => "item/fileChange/requestApproval",
                    _ => "item/commandExecution/requestApproval",
                }
                .into(),
                kind: self.kind,
                thread_id: "thread-a".into(),
                turn_id: "turn-a".into(),
                item_id: "item-a".into(),
                target: self.target.clone().unwrap_or_else(|| match self.kind {
                    NativeApprovalKind::Permissions => NativeApprovalTarget::Permissions {
                        cwd: request.cwd.to_string_lossy().into_owned(),
                    },
                    NativeApprovalKind::FileChange | NativeApprovalKind::LegacyPatch => {
                        NativeApprovalTarget::FileChange {
                            grant_root: Some(request.cwd.to_string_lossy().into_owned()),
                            changes: Vec::new(),
                        }
                    }
                    _ => NativeApprovalTarget::Command {
                        command: "git status".into(),
                        cwd: request.cwd.to_string_lossy().into_owned(),
                    },
                }),
                requested_permissions: (self.kind == NativeApprovalKind::Permissions).then(|| {
                    serde_json::json!({
                        "fileSystem": {"write": [permission_path]}
                    })
                }),
            })
            .await?;
        *self.decision.lock().unwrap() = Some(decision);
        Ok(WorkspaceAgentResponse {
            assistant_text: "approval settled".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

#[tokio::test]
async fn native_approval_wait_is_nonblocking_durable_and_duplicate_safe() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::new(NativeApprovalKind::CommandExecution));
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    save_permission_settings(&service, "full_access", "on_request").await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;

    let pending = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let runs = view(&service).await.runs;
            if let Some(approval) = runs
                .iter()
                .flat_map(|run| &run.native_approvals)
                .find(|approval| approval.status == ait_domain::NativeApprovalStatus::Pending)
            {
                break (approval.run_id.clone(), approval.id.clone());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Reads and event dispatch remain live while the provider task awaits a member decision.
    tokio::time::timeout(Duration::from_millis(500), view(&service))
        .await
        .unwrap();
    let resolved = service
        .execute(Command::ResolveNativeApproval {
            run_id: pending.0.clone(),
            approval_id: pending.1.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    assert!(resolved.ok, "{:?}", resolved.error);
    let duplicate = service
        .execute(Command::ResolveNativeApproval {
            run_id: pending.0,
            approval_id: pending.1,
            action: NativeApprovalAction::Deny,
            scope: None,
        })
        .await;
    assert_eq!(
        duplicate.error.unwrap().code,
        ErrorCode::ToolApprovalRequired
    );
    assert!(running.await.unwrap().ok);
    assert_eq!(
        *agent.decision.lock().unwrap(),
        Some(WorkspaceApprovalDecision::Approved {
            scope: ApprovalGrantScope::OneShot,
            permissions: None,
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_approval_secrets_never_reach_durable_or_reconnected_views() {
    use ait_agent_adapters::codex::CodexAppServerConfig;
    use ait_contracts::ProtocolRequestId;
    use std::os::unix::fs::PermissionsExt as _;

    let fake_server = tempfile::tempdir().unwrap();
    let binary = fake_server.path().join("fake-codex");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
IFS= read -r _ || exit 60
printf '%s\n' '{"id":0,"result":{}}'
IFS= read -r _ || exit 61
IFS= read -r _ || exit 62
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-a"}}}'
IFS= read -r _ || exit 63
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-a"}}}'
printf '%s\n' '{"id":73,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"command-a","command":"curl -H X-Api-Key:header-secret --header=\"Authorization: Bearer auth-secret\" -H \"Cookie: session=cookie-secret\" https://url-user:url-secret@example.test/v1","cwd":"/workspace","reason":"offline fixture"}}'
IFS= read -r approval_response || exit 64
case "$approval_response" in
  *'"id":73'*'"decision":"accept"'*) ;;
  *) exit 65 ;;
esac
printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"answer-a","delta":"approved"}}'
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","items":[],"status":"completed"}}}'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();

    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(
        CodexWorkspaceAgent::from_config(CodexAppServerConfig {
            codex_binary: binary,
            ..CodexAppServerConfig::default()
        })
        .unwrap(),
    );
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    save_permission_settings(&service, "full_access", "on_request").await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    let (run_id, approval_id) = pending_approval(&service).await;

    let persisted = serde_json::to_string(&store.load().await.unwrap().value).unwrap();
    for secret in [
        "header-secret",
        "auth-secret",
        "cookie-secret",
        "url-user",
        "url-secret",
    ] {
        assert!(
            !persisted.contains(secret),
            "secret reached storage: {secret}"
        );
    }

    // A desktop reconnect obtains fresh Project-scoped slices from the same durable daemon state.
    let reconnected_service =
        LocalControlService::with_workspace_agent(store.clone(), agent.clone());
    let reconnected = view(&reconnected_service).await;
    let approval = reconnected
        .runs
        .iter()
        .flat_map(|run| &run.native_approvals)
        .find(|approval| approval.id == approval_id)
        .unwrap();
    assert_eq!(approval.protocol_request_id, ProtocolRequestId::Integer(73));
    assert_eq!(approval.thread_id, "thread-a");
    assert_eq!(approval.turn_id, "turn-a");
    assert_eq!(approval.item_id, "command-a");
    let NativeApprovalTarget::Command { command, .. } = &approval.target else {
        panic!("expected command approval target");
    };
    assert!(command.contains("curl"));
    assert!(command.contains("example.test/v1"));
    assert!(command.contains("X-Api-Key:[REDACTED]"));
    assert!(command.contains("Authorization:[REDACTED]"));
    assert!(command.contains("Cookie:[REDACTED]"));
    for secret in [
        "header-secret",
        "auth-secret",
        "cookie-secret",
        "url-user",
        "url-secret",
    ] {
        assert!(!command.contains(secret));
    }

    let resolved = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id,
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    assert!(resolved.ok, "{:?}", resolved.error);
    let completed = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert!(completed.ok, "{:?}", completed.error);
    let final_view = view(&service).await;
    assert_eq!(
        final_view
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .unwrap()
            .status,
        "completed"
    );
    let final_rendered = format!("{final_view:?}");
    assert!(!final_rendered.contains("header-secret"));
    assert!(!final_rendered.contains("auth-secret"));
    assert!(!final_rendered.contains("cookie-secret"));
    assert!(!final_rendered.contains("url-user"));
    assert!(!final_rendered.contains("url-secret"));
}

#[tokio::test]
async fn denial_is_not_approval_and_session_grants_respect_administrator_policy() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::new(NativeApprovalKind::Permissions));
    let service = Arc::new(
        LocalControlService::with_workspace_agent(store, agent.clone()).with_permission_limits(
            PermissionPolicyLimits {
                max_sandbox: SandboxAccess::FullAccess,
                allow_session_approvals: false,
            },
        ),
    );
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(approval) = view(&service)
                .await
                .runs
                .iter()
                .flat_map(|run| &run.native_approvals)
                .find(|approval| approval.status == ait_domain::NativeApprovalStatus::Pending)
            {
                break (approval.run_id.clone(), approval.id.clone());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let wrong_scope = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    assert_eq!(
        wrong_scope.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    let prohibited = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::Session),
        })
        .await;
    assert_eq!(
        prohibited.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    let denied = service
        .execute(Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action: NativeApprovalAction::Deny,
            scope: None,
        })
        .await;
    assert!(denied.ok, "{:?}", denied.error);
    assert!(running.await.unwrap().ok);
    assert_eq!(
        *agent.decision.lock().unwrap(),
        Some(WorkspaceApprovalDecision::Denied)
    );
}

#[tokio::test]
async fn permission_write_grants_cannot_exceed_run_or_administrator_ceiling() {
    reject_permission_write_for_read_only_run(SandboxAccess::ReadOnly).await;
    reject_permission_write_for_read_only_run(SandboxAccess::FullAccess).await;
}

async fn reject_permission_write_for_read_only_run(max_sandbox: SandboxAccess) {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::new(NativeApprovalKind::Permissions));
    let service = Arc::new(
        LocalControlService::with_workspace_agent(store, agent.clone()).with_permission_limits(
            PermissionPolicyLimits {
                max_sandbox,
                allow_session_approvals: true,
            },
        ),
    );
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = pending_approval(&service).await;
    let rejected = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::Turn),
        })
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    let pending = view(&service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    assert_eq!(
        pending.native_approvals[0].status,
        ait_domain::NativeApprovalStatus::Pending
    );
    assert!(pending.native_approvals[0].granted_permissions.is_none());
    assert!(agent.decision.lock().unwrap().is_none());
    let cancelled = service
        .execute(Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action: NativeApprovalAction::Cancel,
            scope: None,
        })
        .await;
    assert!(cancelled.ok, "{:?}", cancelled.error);
    tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn workspace_write_permission_grants_cannot_escape_the_project() {
    let outside = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::permissions_at(
        outside.path().join("escaped.txt").to_string_lossy(),
    ));
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    save_permission_settings(&service, "workspace_write", "on_request").await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = pending_approval(&service).await;
    let rejected = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::Turn),
        })
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert!(!outside.path().join("escaped.txt").exists());
    let pending = view(&service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    assert_eq!(
        pending.native_approvals[0].status,
        ait_domain::NativeApprovalStatus::Pending
    );
    assert!(pending.native_approvals[0].granted_permissions.is_none());
    let cancelled = service
        .execute(Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action: NativeApprovalAction::Cancel,
            scope: None,
        })
        .await;
    assert!(cancelled.ok, "{:?}", cancelled.error);
    tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
}

async fn pending_approval(service: &LocalControlService) -> (String, String) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(approval) = view(service)
                .await
                .runs
                .iter()
                .flat_map(|run| &run.native_approvals)
                .find(|approval| approval.status == ait_domain::NativeApprovalStatus::Pending)
            {
                break (approval.run_id.clone(), approval.id.clone());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn cancelling_each_native_approval_kind_cancels_the_run_without_hanging() {
    for kind in [
        NativeApprovalKind::CommandExecution,
        NativeApprovalKind::FileChange,
        NativeApprovalKind::Permissions,
    ] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let agent = Arc::new(ApprovalAgent::new(kind));
        let service = Arc::new(LocalControlService::with_workspace_agent(
            store,
            agent.clone(),
        ));
        let _directory = setup(&service, config("high")).await;
        let running = {
            let service = service.clone();
            tokio::spawn(async move { service.execute(send("one")).await })
        };
        wait_for_signal(&agent.requested).await;
        let (run_id, approval_id) = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(run) = view(&service)
                    .await
                    .runs
                    .into_iter()
                    .find(|run| !run.native_approvals.is_empty())
                {
                    break (run.id, run.native_approvals[0].id.clone());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let cancelled = service
            .execute(Command::ResolveNativeApproval {
                run_id: run_id.clone(),
                approval_id: approval_id.clone(),
                action: NativeApprovalAction::Cancel,
                scope: None,
            })
            .await;
        assert!(cancelled.ok, "{kind:?}: {:?}", cancelled.error);
        tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            *agent.decision.lock().unwrap(),
            Some(WorkspaceApprovalDecision::Cancelled),
            "{kind:?}"
        );
        let snapshot = view(&service).await;
        let run = snapshot
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .unwrap();
        assert_eq!(run.status, "cancelled", "{kind:?}");
        assert_eq!(
            run.native_approvals[0].status,
            ait_domain::NativeApprovalStatus::Cancelled,
            "{kind:?}"
        );
        assert!(
            snapshot
                .messages
                .iter()
                .all(|message| message.role != "assistant"),
            "{kind:?}"
        );
        let duplicate = service
            .execute(Command::ResolveNativeApproval {
                run_id,
                approval_id,
                action: NativeApprovalAction::Approve,
                scope: Some(if kind == NativeApprovalKind::Permissions {
                    ApprovalGrantScope::Turn
                } else {
                    ApprovalGrantScope::OneShot
                }),
            })
            .await;
        assert_eq!(
            duplicate.error.unwrap().code,
            ErrorCode::RunAlreadyTerminal,
            "{kind:?}"
        );
    }
}

// NEC-192: approvals must never upgrade the immutable Run sandbox.
#[tokio::test]
async fn file_and_command_approvals_respect_each_run_sandbox() {
    for sandbox in ["read_only", "workspace_write", "full_access"] {
        for kind in [
            NativeApprovalKind::FileChange,
            NativeApprovalKind::LegacyPatch,
            NativeApprovalKind::CommandExecution,
            NativeApprovalKind::LegacyCommand,
        ] {
            let agent = Arc::new(ApprovalAgent::new(kind));
            let service = Arc::new(LocalControlService::with_workspace_agent(
                Arc::new(SqliteControlStore::in_memory().unwrap()),
                agent.clone(),
            ));
            let _directory = setup(&service, config("high")).await;
            save_permission_settings(&service, sandbox, "on_request").await;
            let allowed = sandbox == "full_access"
                || (sandbox == "workspace_write"
                    && matches!(
                        kind,
                        NativeApprovalKind::FileChange | NativeApprovalKind::LegacyPatch
                    ));
            check_approval_grant(&service, &agent, allowed).await;
        }
    }
}

async fn check_approval_grant(
    service: &Arc<LocalControlService>,
    agent: &Arc<ApprovalAgent>,
    allowed: bool,
) {
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = pending_approval(service).await;
    let response = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    // Always finish the provider task, including when the assertion will fail.
    let approval = view(service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap()
        .native_approvals
        .remove(0);
    if !response.ok {
        ok(
            service,
            Command::ResolveNativeApproval {
                run_id,
                approval_id,
                action: NativeApprovalAction::Deny,
                scope: None,
            },
        )
        .await;
    }
    let result = tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(response.ok, allowed, "{:?}: {response:?}", agent.kind);
    if allowed {
        assert_eq!(approval.status, ait_domain::NativeApprovalStatus::Approved);
    } else {
        assert_eq!(
            response.error.unwrap().code,
            ErrorCode::InvalidConfiguration
        );
        assert_eq!(approval.status, ait_domain::NativeApprovalStatus::Pending);
        assert!(approval.granted_scope.is_none());
        assert!(approval.granted_permissions.is_none());
        assert_eq!(
            *agent.decision.lock().unwrap(),
            Some(WorkspaceApprovalDecision::Denied)
        );
    }
}

#[tokio::test]
async fn workspace_file_grants_reject_outside_and_ambiguous_paths() {
    for kind in [
        NativeApprovalKind::FileChange,
        NativeApprovalKind::LegacyPatch,
    ] {
        for path in [
            "../escaped.txt",
            "nested/../../escaped.txt",
            "nested/../safe.txt",
        ] {
            let mut fixture = ApprovalAgent::new(kind);
            fixture.target = Some(NativeApprovalTarget::FileChange {
                grant_root: None,
                changes: vec![ait_domain::NativeApprovalFileChange {
                    path: path.into(),
                    kind: ait_domain::NativeApprovalFileChangeKind::Add,
                }],
            });
            let agent = Arc::new(fixture);
            let service = Arc::new(LocalControlService::with_workspace_agent(
                Arc::new(SqliteControlStore::in_memory().unwrap()),
                agent.clone(),
            ));
            let _directory = setup(&service, config("high")).await;
            save_permission_settings(&service, "workspace_write", "on_request").await;
            check_approval_grant(&service, &agent, false).await;
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_file_grants_reject_symlinks_including_dangling_targets() {
    for dangling in [false, true] {
        let outside = tempfile::tempdir().unwrap();
        let mut fixture = ApprovalAgent::new(NativeApprovalKind::FileChange);
        fixture.target = Some(NativeApprovalTarget::FileChange {
            grant_root: Some("link".into()),
            changes: Vec::new(),
        });
        let agent = Arc::new(fixture);
        let service = Arc::new(LocalControlService::with_workspace_agent(
            Arc::new(SqliteControlStore::in_memory().unwrap()),
            agent.clone(),
        ));
        let directory = setup(&service, config("high")).await;
        let target = if dangling {
            outside.path().join("missing")
        } else {
            outside.path().to_path_buf()
        };
        std::os::unix::fs::symlink(target, directory.path().join("link")).unwrap();
        for args in [
            &["add", "link"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture link",
            ][..],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(directory.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        save_permission_settings(&service, "workspace_write", "on_request").await;
        check_approval_grant(&service, &agent, false).await;
    }
}
