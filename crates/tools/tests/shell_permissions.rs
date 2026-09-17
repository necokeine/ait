//! Real shell permission, output and descendant-lifecycle regressions.
#![cfg(unix)]
#![allow(clippy::pedantic)]
use ait_domain::{ErrorCode, RunId, RunPermissionProfile, SandboxAccess, ToolExecutionId};
use ait_ports::{RunToolFactory, ToolInvocation};
use ait_tools::host::{HostToolFactory, MAX_BYTES};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn call(command: String) -> ToolInvocation {
    ToolInvocation {
        run_id: RunId::new("run"),
        call_id: "call".into(),
        execution_id: ToolExecutionId::new("execution"),
        tool_name: "bash".into(),
        arguments: json!({"command":command,"description":"Exercise shell permissions"}),
        message_path: Vec::new(),
        cancellation: CancellationToken::new(),
    }
}
fn quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[tokio::test]
async fn applies_all_three_profiles_to_real_commands_and_explicit_requests() {
    for sandbox in [
        SandboxAccess::ReadOnly,
        SandboxAccess::WorkspaceWrite,
        SandboxAccess::FullAccess,
    ] {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // Production Session worktrees live beneath a Project .ait directory.
        std::fs::create_dir_all(root.path().join(".ait/session")).unwrap();
        let path = root.path().join(".ait/session").canonicalize().unwrap();
        std::fs::create_dir(path.join(".git")).unwrap();
        std::fs::write(path.join("source"), "one\ntwo\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), path.join("link")).unwrap();
        let tools = HostToolFactory
            .create(
                &path,
                RunPermissionProfile {
                    sandbox,
                    ..Default::default()
                },
            )
            .unwrap();
        if !tools.executable_tools().contains(&"bash".to_owned()) {
            assert!(
                std::env::var_os("AIT_REQUIRE_SHELL_SANDBOX").is_none(),
                "required sandbox failed its real startup probe"
            );
            continue;
        }
        let inspect = tools
            .execute(call("pwd && find . -type f | wc -l && wc -l source".into()))
            .await
            .unwrap()
            .output;
        assert_eq!(inspect["exit_status"], 0, "{inspect}");
        assert!(
            inspect["stdout"]
                .as_str()
                .unwrap()
                .contains(path.to_str().unwrap())
        );
        for (target, allowed) in [
            (path.join("written"), sandbox != SandboxAccess::ReadOnly),
            (
                outside.path().join("outside"),
                sandbox == SandboxAccess::FullAccess,
            ),
            (
                path.join("link/escape"),
                sandbox == SandboxAccess::FullAccess,
            ),
            (
                path.join(".git/config"),
                sandbox == SandboxAccess::FullAccess,
            ),
        ] {
            let result = tools
                .execute(call(format!("printf hello > {}", quote(&target))))
                .await
                .unwrap()
                .output;
            assert_eq!(
                result["exit_status"] == 0,
                allowed,
                "{sandbox:?}: {target:?}: {result}"
            );
            assert_eq!(target.exists(), allowed);
        }
        let mut explicit = call("printf allowed".into());
        explicit.arguments["sandbox_permissions"] = json!("workspace-write");
        assert_eq!(
            tools.requires_approval("bash", &explicit.arguments),
            sandbox == SandboxAccess::ReadOnly
        );
        assert_eq!(
            tools.execute(explicit).await.is_ok(),
            sandbox != SandboxAccess::ReadOnly
        );
        assert!(!tools.parallel_safe("bash", &json!({"command":"printf ok"})));
        let mut workdir = call("pwd".into());
        workdir.arguments["workdir"] = json!(outside.path());
        assert_eq!(
            tools.execute(workdir).await.is_ok(),
            sandbox == SandboxAccess::FullAccess
        );
        // Loopback is network access too: restricted profiles cannot reach the host listener.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let result = tools
            .execute(call(format!(
                "echo hello > /dev/tcp/127.0.0.1/{}",
                listener.local_addr().unwrap().port()
            )))
            .await
            .unwrap()
            .output;
        assert_eq!(
            result["exit_status"] == 0,
            sandbox == SandboxAccess::FullAccess,
            "{result}"
        );
        assert_eq!(
            listener.accept().is_ok(),
            sandbox == SandboxAccess::FullAccess
        );
        let socket_path = outside.path().join("host.sock");
        let socket = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        socket.set_nonblocking(true).unwrap();
        let script = format!(
            "import socket; s=socket.socket(socket.AF_UNIX); s.connect({}); s.close()",
            serde_json::to_string(socket_path.to_str().unwrap()).unwrap()
        );
        let command = format!("python3 -c '{}'", script.replace('\'', "'\\''"));
        let result = tools.execute(call(command)).await.unwrap().output;
        assert_eq!(
            result["exit_status"] == 0,
            sandbox == SandboxAccess::FullAccess,
            "{result}"
        );
        assert_eq!(
            socket.accept().is_ok(),
            sandbox == SandboxAccess::FullAccess
        );
    }
}

#[tokio::test]
async fn outside_secrets_never_enter_restricted_shell_results() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let home = tempfile::Builder::new()
        .prefix("ait-shell-read-test-")
        .tempdir_in(std::env::var_os("HOME").unwrap())
        .unwrap();
    let session = project.path().join(".ait/session");
    std::fs::create_dir_all(&session).unwrap();
    let session = session.canonicalize().unwrap();
    let marker = "NEC263_PRIVATE_READ_MARKER_7a68409b";
    let secrets = [
        outside.path().join("secret"),
        home.path().join("credential"),
        project.path().join(".ait/other-session-secret"),
    ];
    for path in &secrets {
        std::fs::write(path, marker).unwrap();
    }
    std::fs::write(session.join("source"), "workspace-readable").unwrap();
    std::os::unix::fs::symlink(&secrets[0], session.join("linked-secret")).unwrap();
    std::os::unix::fs::symlink(home.path(), session.join("linked-home")).unwrap();
    // Metadata protection must not mount a private symlink target into Linux's
    // otherwise empty root when Workspace Write is selected.
    std::os::unix::fs::symlink(outside.path(), session.join(".git")).unwrap();
    std::os::unix::fs::symlink(home.path(), session.join(".ait")).unwrap();
    let targets = secrets.into_iter().chain([
        session.join("linked-secret"),
        session.join("linked-home/credential"),
        session.join(".git/secret"),
        session.join(".ait/credential"),
    ]);
    for target in targets {
        for sandbox in [
            SandboxAccess::ReadOnly,
            SandboxAccess::WorkspaceWrite,
            SandboxAccess::FullAccess,
        ] {
            let tools = HostToolFactory
                .create(
                    &session,
                    RunPermissionProfile {
                        sandbox,
                        ..Default::default()
                    },
                )
                .unwrap();
            if !tools.executable_tools().contains(&"bash".to_owned()) {
                assert!(
                    std::env::var_os("AIT_REQUIRE_SHELL_SANDBOX").is_none(),
                    "required sandbox unavailable"
                );
                continue;
            }
            let source = tools
                .execute(call("/bin/cat source".into()))
                .await
                .unwrap()
                .output;
            assert_eq!(source["exit_status"], 0, "{source}");
            assert_eq!(source["stdout"], "workspace-readable");
            let output = tools
                .execute(call(format!("/bin/cat {}", quote(&target))))
                .await
                .unwrap()
                .output;
            let full = sandbox == SandboxAccess::FullAccess;
            assert_eq!(
                output["exit_status"] == 0,
                full,
                "{sandbox:?} {target:?}: {output}"
            );
            assert_eq!(
                output.to_string().contains(marker),
                full,
                "{sandbox:?}: {output}"
            );
        }
    }
}

#[tokio::test]
async fn captures_failure_stderr_and_bounds_both_streams() {
    let root = tempfile::tempdir().unwrap();
    let tools = HostToolFactory
        .create(
            &root.path().canonicalize().unwrap(),
            RunPermissionProfile {
                sandbox: SandboxAccess::FullAccess,
                ..Default::default()
            },
        )
        .unwrap();
    let result = tools
        .execute(call("printf out; printf err >&2; exit 7".into()))
        .await
        .unwrap()
        .output;
    assert_eq!(result["stdout"], "out");
    assert_eq!(result["stderr"], "err");
    assert_eq!(result["exit_status"], 7);
    let result = tools
        .execute(call("printf '%100000s' x; printf '%100000s' y >&2".into()))
        .await
        .unwrap()
        .output;
    assert_eq!(result["stdout_truncated"], true);
    assert_eq!(result["stderr_truncated"], true);
    assert!(result.to_string().len() < MAX_BYTES);
}

#[tokio::test]
async fn cancels_descendants_before_drain_returns_and_cleans_up_after_normal_exit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let tools = HostToolFactory
        .create(
            &path,
            RunPermissionProfile {
                sandbox: SandboxAccess::FullAccess,
                ..Default::default()
            },
        )
        .unwrap();
    let request = call("echo ready > ready; (sleep 0.5; echo escaped > escaped) & wait".into());
    let cancellation = request.cancellation.clone();
    let running = tools.clone();
    let task = tokio::spawn(async move { running.execute(request).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !path.join("ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    assert_eq!(
        task.await.unwrap().unwrap_err().code,
        ErrorCode::RunCancelled
    );
    tools.cancel_and_drain().await;
    let second = HostToolFactory
        .create(
            &path,
            RunPermissionProfile {
                sandbox: SandboxAccess::FullAccess,
                ..Default::default()
            },
        )
        .unwrap();
    second
        .execute(call(
            "(sleep 0.5; echo leaked > leaked) >/dev/null 2>&1 & echo done".into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(650)).await;
    assert!(!path.join("escaped").exists());
    assert!(!path.join("leaked").exists());
}
