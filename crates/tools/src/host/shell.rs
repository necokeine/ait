//! Execute real shell syntax under the immutable Run permission.
use super::{HostTools, MAX_BYTES, denied, failed, string};
use ait_domain::{DomainError, ErrorCode, SandboxAccess};
use ait_ports::ToolInvocation;
use process_wrap::tokio::{CommandWrap, KillOnDrop};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};

impl HostTools {
    pub(super) fn shell_available(&self) -> bool {
        cfg!(unix)
            && Path::new("/bin/bash").is_file()
            && (self.profile.sandbox == SandboxAccess::FullAccess || sandbox_binary().is_some())
    }

    fn shell_command(&self, cwd: &Path) -> Result<CommandWrap, DomainError> {
        if !self.shell_available() {
            return Err(denied());
        }
        let mut command = CommandWrap::with_new("/bin/bash", |_| {});
        let builder = command.command_mut();
        if self.profile.sandbox == SandboxAccess::FullAccess {
            *builder = tokio::process::Command::new("/bin/bash");
        } else {
            *builder = tokio::process::Command::new(sandbox_binary().ok_or_else(denied)?);
            #[cfg(target_os = "macos")]
            {
                builder.args(["-p", MACOS_POLICY]);
                // Parameters are passed as argv, never interpolated into policy source.
                builder
                    .arg("-D")
                    .arg(format!("WORKSPACE={}", self.root_path.display()));
                builder.arg("-D").arg(format!(
                    "GIT_METADATA={}",
                    self.root_path.join(".git").display()
                ));
                builder.arg("-D").arg(format!(
                    "AIT_METADATA={}",
                    self.root_path.join(".ait").display()
                ));
                builder.arg("-D").arg(format!(
                    "WRITABLE={}",
                    if self.profile.sandbox == SandboxAccess::WorkspaceWrite {
                        "yes"
                    } else {
                        "no"
                    }
                ));
            }
            #[cfg(target_os = "linux")]
            {
                // An independent PID/network/IPC namespace plus a read-only host
                // filesystem; only the selected workspace is mounted writable.
                builder.args([
                    "--unshare-all",
                    "--die-with-parent",
                    "--new-session",
                    "--ro-bind",
                    "/",
                    "/",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                ]);
                if self.profile.sandbox == SandboxAccess::WorkspaceWrite {
                    builder
                        .arg("--bind")
                        .arg(&self.root_path)
                        .arg(&self.root_path);
                    for name in [".git", ".ait"] {
                        let path = self.root_path.join(name);
                        if path.exists() {
                            builder.arg("--ro-bind").arg(&path).arg(&path);
                        }
                    }
                }
                builder.arg("--chdir").arg(cwd).arg("--");
            }
            builder.arg("/bin/bash");
        }
        builder
            .args(["--noprofile", "--norc", "-c"])
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()),
            )
            .env("HOME", &self.root_path)
            .env("LANG", "en_US.UTF-8")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        Ok(command)
    }

    pub(super) async fn shell(&self, request: &ToolInvocation) -> Result<Value, DomainError> {
        self.check(request)?;
        let path = request
            .arguments
            .get("workdir")
            .and_then(Value::as_str)
            .unwrap_or(".");
        let cwd = self
            .root_path
            .join(path)
            .canonicalize()
            .map_err(|_| denied())?;
        if !cwd.is_dir()
            || (self.profile.sandbox != SandboxAccess::FullAccess
                && !cwd.starts_with(&self.root_path))
        {
            return Err(denied());
        }
        let timeout = request
            .arguments
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(10_000)
            .clamp(1, 120_000);
        let mut command = self.shell_command(&cwd)?;
        command
            .command_mut()
            .arg(string(&request.arguments, "command")?);
        self.check(request)?;
        let mut child = command.spawn().map_err(|_| failed())?;
        let stdout = child.stdout().take().ok_or_else(failed)?;
        let stderr = child.stderr().take().ok_or_else(failed)?;
        let work = async {
            let ((stdout, stdout_truncated), (stderr, stderr_truncated), status) =
                tokio::try_join!(capture(stdout), capture(stderr), async {
                    child.wait().await.map_err(|_| failed())
                })?;
            Ok(
                json!({"stdout":stdout,"stderr":stderr,"exit_status":status.code(),"stdout_truncated":stdout_truncated,"stderr_truncated":stderr_truncated,"truncated":stdout_truncated || stderr_truncated}),
            )
        };
        let result = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => Err(DomainError::invariant(ErrorCode::RunCancelled,"tool cancelled")),
            () = self.workers.stopping.cancelled() => Err(DomainError::invariant(ErrorCode::RunCancelled,"host tools stopped")),
            result = tokio::time::timeout(Duration::from_millis(timeout), work) => result.unwrap_or_else(|_| Err(DomainError::invariant(ErrorCode::RunLimitExceeded,"tool timeout elapsed"))),
        };
        // Always terminate the group, including any background descendants left
        // after the shell exits. Collect the child before releasing the worker guard.
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }
}

fn sandbox_binary() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let candidates = ["/usr/bin/sandbox-exec"];
    #[cfg(target_os = "linux")]
    let candidates = ["/usr/bin/bwrap", "/bin/bwrap"];
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let candidates: [&str; 0] = [];
    candidates
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

async fn capture(mut pipe: impl AsyncRead + Unpin) -> Result<(String, bool), DomainError> {
    // Worst-case JSON escaping expands a byte by six. Keep both streams inside
    // the host result budget, and drain excess output without accumulating it.
    let limit = MAX_BYTES / 16;
    let mut output = Vec::new();
    let mut buffer = vec![0; 8192];
    let mut truncated = false;
    loop {
        let count = pipe.read(&mut buffer).await.map_err(|_| failed())?;
        if count == 0 {
            break;
        }
        let retained = count.min(limit - output.len());
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < count;
    }
    Ok((String::from_utf8_lossy(&output).into_owned(), truncated))
}

#[cfg(target_os = "macos")]
const MACOS_POLICY: &str = r#"
(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow file-read*)
(allow file-write-data (literal "/dev/null"))
(if (string=? (param "WRITABLE") "yes")
    (allow file-write* (subpath (param "WORKSPACE"))))
(deny file-write* (subpath (param "GIT_METADATA")) (subpath (param "AIT_METADATA")))
"#;
