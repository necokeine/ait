use super::*;
use crate::control::workspace_tests::FakeWorkspace;

#[tokio::test]
async fn facts_cannot_raise_run_or_administrator_write_ceilings() {
    let workspace = FakeWorkspace::default();
    let permissions = NativePermissionProfile {
        file_system: Some(ait_contracts::NativeFileSystemPermissions {
            write: vec!["inside".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    for (sandbox, maximum) in [
        (SandboxAccess::ReadOnly, SandboxAccess::FullAccess),
        (SandboxAccess::WorkspaceWrite, SandboxAccess::ReadOnly),
    ] {
        let run = RunPermissionProfile {
            sandbox,
            approval: ait_domain::ApprovalMode::OnRequest,
        };
        let limits = PermissionPolicyLimits {
            max_sandbox: maximum,
            allow_session_approvals: true,
        };
        assert!(
            validate_permission_grant_ceiling(
                &workspace,
                &permissions,
                run,
                limits,
                Path::new(if cfg!(windows) {
                    "C:/alias/project"
                } else {
                    "/alias/project"
                })
            )
            .await
            .is_err()
        );
    }
    assert!(workspace.trace.lock().unwrap().is_empty());
    let run = RunPermissionProfile {
        sandbox: SandboxAccess::WorkspaceWrite,
        approval: ait_domain::ApprovalMode::OnRequest,
    };
    assert!(
        validate_permission_grant_ceiling(
            &workspace,
            &permissions,
            run,
            PermissionPolicyLimits::default(),
            Path::new(if cfg!(windows) {
                "C:/alias/project"
            } else {
                "/alias/project"
            })
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn lexical_escape_is_rejected_before_facts_and_canonical_escape_after_facts() {
    let workspace = FakeWorkspace::default();
    for path in ["../escape", "symlink/../escape", "/elsewhere/file"] {
        assert!(
            ensure_permission_path_in_project(
                &workspace,
                path,
                Path::new(if cfg!(windows) {
                    "C:/alias/project"
                } else {
                    "/alias/project"
                })
            )
            .await
            .is_err()
        );
    }
    assert!(workspace.trace.lock().unwrap().is_empty());
    assert!(
        ensure_permission_path_in_project(
            &workspace,
            "new/file",
            Path::new(if cfg!(windows) {
                "C:/alias/project"
            } else {
                "/alias/project"
            })
        )
        .await
        .is_ok()
    );
    workspace
        .facts
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .canonical_existing = if cfg!(windows) {
        "C:/outside"
    } else {
        "/outside"
    }
    .into();
    assert!(
        ensure_permission_path_in_project(
            &workspace,
            "new/file",
            Path::new(if cfg!(windows) {
                "C:/alias/project"
            } else {
                "/alias/project"
            })
        )
        .await
        .is_err()
    );
    *workspace.facts.lock().unwrap() = Err(DomainError::invariant(
        ErrorCode::ProjectPathNotFound,
        "unresolvable",
    ));
    assert!(
        ensure_permission_path_in_project(
            &workspace,
            "dangling/file",
            Path::new(if cfg!(windows) {
                "C:/alias/project"
            } else {
                "/alias/project"
            })
        )
        .await
        .is_err()
    );
}
