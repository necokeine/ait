use super::*;
use ait_domain::NativeNetworkProtocol;

#[test]
fn network_approval_projection_is_network_specific_and_bounded() {
    assert_eq!(
        approval_target(
            ApprovalKind::CommandExecution,
            &json!({
                "networkApprovalContext": {"host": "api.example.test", "protocol": "https"}
            }),
            &HashMap::new(),
        )
        .unwrap(),
        NativeApprovalTarget::Network {
            host: "api.example.test".into(),
            protocol: NativeNetworkProtocol::Https,
        }
    );
    assert!(
        approval_target(
            ApprovalKind::CommandExecution,
            &json!({
                "networkApprovalContext": {"host": "api.example.test", "protocol": "ftp"}
            }),
            &HashMap::new(),
        )
        .is_err()
    );
}

#[test]
fn command_approval_projection_redacts_credentials() {
    for (command, secrets) in [
        (
            "curl --api-key=very-secret https://url-user:password@example.test/v1",
            &["very-secret", "url-user", "password"][..],
        ),
        (
            "curl -H X-Api-Key:header-secret https://example.test/v1",
            &["header-secret"][..],
        ),
        (
            "curl -H \"Authorization: Bearer auth-secret\" https://example.test/v1",
            &["auth-secret"][..],
        ),
        (
            "curl --header='Cookie: session=cookie-secret' https://example.test/v1",
            &["cookie-secret"][..],
        ),
        (
            "curl --header=\"Authorization:Bearer opaque-value\" https://example.test/v1",
            &["opaque-value"][..],
        ),
    ] {
        let target = approval_target(
            ApprovalKind::CommandExecution,
            &json!({"command": command, "cwd": "/workspace"}),
            &HashMap::new(),
        )
        .unwrap();
        let NativeApprovalTarget::Command { command, cwd } = target else {
            panic!("expected a command target");
        };
        for secret in secrets {
            assert!(!command.contains(secret), "secret leaked from {command:?}");
        }
        assert!(command.contains("curl"));
        assert!(command.contains("example.test"));
        assert!(command.contains("[REDACTED]"));
        assert_eq!(cwd, "/workspace");
    }

    let array_target = approval_target(
        ApprovalKind::CommandExecution,
        &json!({
            "command": [
                "curl",
                "--header",
                "Authorization: Bearer array-secret",
                "https://array-user:array-password@example.test/v1"
            ],
            "cwd": "/workspace"
        }),
        &HashMap::new(),
    )
    .unwrap();
    let NativeApprovalTarget::Command { command, .. } = array_target else {
        panic!("expected a command target");
    };
    assert_eq!(
        command,
        "curl --header Authorization:[REDACTED] https://[REDACTED]@example.test/v1"
    );
    for secret in ["array-secret", "array-user", "array-password"] {
        assert!(!command.contains(secret));
    }
}

#[test]
fn command_approval_projection_fails_closed_on_ambiguous_syntax() {
    for command in [
        "curl -H \"Authorization: Bearer unfinished",
        "curl --api-key",
        "curl --header=Authorization:Bearer opaque-value https://example.test/v1",
        "curl Authorization: Bearer opaque-value https://example.test/v1",
        "curl -H Cookie: first=opaque-cookie https://example.test/v1",
        "curl -- -H Authorization: /etc/passwd",
    ] {
        assert!(
            approval_target(
                ApprovalKind::CommandExecution,
                &json!({"command": command, "cwd": "/workspace"}),
                &HashMap::new(),
            )
            .is_err(),
            "unsafe command projection was accepted: {command}"
        );
    }
}

#[test]
fn command_approval_projection_never_invents_header_argument_boundaries() {
    for command in [
        json!("grep -H Authorization: /etc/passwd"),
        json!(["grep", "-H", "Authorization:", "/etc/passwd"]),
    ] {
        assert!(
            approval_target(
                ApprovalKind::CommandExecution,
                &json!({"command": command, "cwd": "/workspace"}),
                &HashMap::new(),
            )
            .is_err(),
            "an ambiguous argument was silently omitted"
        );
    }

    for command in [
        json!("curl -H Authorization:value /etc/passwd"),
        json!(["curl", "-H", "Authorization:value", "/etc/passwd"]),
    ] {
        let target = approval_target(
            ApprovalKind::CommandExecution,
            &json!({"command": command, "cwd": "/workspace"}),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            target,
            NativeApprovalTarget::Command {
                command: "curl -H Authorization:[REDACTED] /etc/passwd".into(),
                cwd: "/workspace".into(),
            }
        );
    }

    assert_eq!(
        approval_target(
            ApprovalKind::CommandExecution,
            &json!({
                "command": ["grep", "-H", "needle", "/etc/file with spaces"],
                "cwd": "/workspace"
            }),
            &HashMap::new(),
        )
        .unwrap(),
        NativeApprovalTarget::Command {
            command: "grep -H needle \"/etc/file with spaces\"".into(),
            cwd: "/workspace".into(),
        }
    );
}

#[test]
fn cancel_is_never_encoded_as_an_approval_or_empty_permission_grant() {
    for method in [
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "item/permissions/requestApproval",
    ] {
        let failure = approval_response(method, ApprovalDecision::Cancel).unwrap_err();
        assert_eq!(failure.kind, AdapterErrorKind::Cancelled, "{method}");
    }
}

#[cfg(windows)]
#[test]
fn capability_binding_rejects_windows_junction_ancestors() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("sentinel.txt"),
        b"outside exact\0bytes\n",
    )
    .unwrap();
    let junction = root.path().join("junction");
    let status = ProcessCommand::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(outside.path())
        .status()
        .unwrap();
    assert!(status.success(), "cannot create Windows junction fixture");

    let capability = open_bound_root(root.path()).unwrap();
    let failure = BoundPath::bind(root.path(), &capability, Path::new("junction/sentinel.txt"))
        .err()
        .expect("junction ancestor must be rejected");

    assert_eq!(failure.code, ErrorCode::RunRecoveryFailed);
    assert_eq!(
        fs::read(outside.path().join("sentinel.txt")).unwrap(),
        b"outside exact\0bytes\n"
    );
}

#[cfg(unix)]
#[test]
fn failed_partial_worktree_cleanup_keeps_the_recovery_ref_and_reports_the_handle() {
    use std::os::unix::fs::PermissionsExt as _;

    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init"]).unwrap();
    git(
        repository.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    )
    .unwrap();
    let baseline = git_head(repository.path()).unwrap();
    let run_ref = "refs/ait/runs/cleanup-failure";
    git(repository.path(), &["update-ref", run_ref, &baseline]).unwrap();
    let partial = repository.path().join("partial-worktree");
    git(
        repository.path(),
        &[
            "worktree",
            "add",
            "--detach",
            partial.to_str().unwrap(),
            &baseline,
        ],
    )
    .unwrap();
    let original_permissions = fs::metadata(repository.path()).unwrap().permissions();
    let mut unwritable = original_permissions.clone();
    unwritable.set_mode(0o555);
    fs::set_permissions(repository.path(), unwritable).unwrap();

    let failure = settle_setup_failure(
        repository.path(),
        &partial,
        run_ref,
        domain_error(
            ErrorCode::ProjectGitInitFailed,
            "injected setup failure",
            false,
        ),
    );
    fs::set_permissions(repository.path(), original_permissions).unwrap();

    assert_eq!(failure.code, ErrorCode::RunRecoveryFailed);
    assert!(failure.message.contains("partial isolated workspace"));
    assert!(failure.message.contains(run_ref));
    assert!(git_ref_exists(repository.path(), run_ref).unwrap());
    assert!(partial.exists());
}
