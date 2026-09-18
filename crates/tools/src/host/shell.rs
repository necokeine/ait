//! Execute real shell syntax under the immutable Run permission.
use super::{HostTools, MAX_BYTES, denied, failed, string};
use ait_domain::{DomainError, ErrorCode, SandboxAccess};
use ait_ports::ToolInvocation;
use process_wrap::tokio::{CommandWrap, KillOnDrop};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

mod backend;
pub(super) use backend::ShellBackend;

impl HostTools {
    pub(super) fn shell_available(&self) -> bool {
        self.shell_backend.is_some()
    }

    fn shell_command(&self, cwd: &Path) -> Result<CommandWrap, DomainError> {
        let builder = self.shell_backend.as_ref().ok_or_else(denied)?.command(
            &self.root_path,
            self.profile.sandbox,
            cwd,
        )?;
        let mut command = CommandWrap::with_new("/bin/bash", |_| {});
        *command.command_mut() = tokio::process::Command::from(builder);
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
            Ok(json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_status": status.code(),
                "stdout_truncated": stdout_truncated,
                "stderr_truncated": stderr_truncated,
                "truncated": stdout_truncated || stderr_truncated
            }))
        };
        let result = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                Err(DomainError::invariant(ErrorCode::RunCancelled, "tool cancelled"))
            },
            () = self.workers.stopping.cancelled() => {
                Err(DomainError::invariant(ErrorCode::RunCancelled, "host tools stopped"))
            },
            result = tokio::time::timeout(Duration::from_millis(timeout), work) => {
                result.unwrap_or_else(|_| {
                    Err(DomainError::invariant(
                        ErrorCode::RunLimitExceeded,
                        "tool timeout elapsed",
                    ))
                })
            },
        };
        // Always terminate the group, including any background descendants left
        // after the shell exits. Collect the child before releasing the worker guard.
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }
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
