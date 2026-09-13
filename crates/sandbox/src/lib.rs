//! Local process and filesystem isolation adapters.

use ait_domain::{DomainError, ErrorCode, RunPermissionProfile, SandboxAccess};
use ait_ports::{RunTool, RunToolFactory};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{path::Path, process::Stdio, sync::Arc};

/// Immutable administrator ceiling; approval never mutates this factory.
pub struct SandboxToolFactory {
    /// Maximum profile accepted by the administrator.
    pub maximum: SandboxAccess,
}
impl SandboxToolFactory {
    /// Create a capability executor with a bootstrap-frozen output/concurrency cap.
    /// # Errors
    /// Rejects invalid bounds or permissions above the administrator ceiling.
    pub fn create_bounded(
        &self,
        root: &Path,
        profile: RunPermissionProfile,
        output_bytes: u32,
        concurrency: u16,
    ) -> Result<Arc<dyn RunTool>, DomainError> {
        if output_bytes == 0 || output_bytes > 65_536 || concurrency == 0 || concurrency > 4 {
            return Err(DomainError::invariant(
                ErrorCode::RunLimitExceeded,
                "invalid tool limits",
            ));
        }
        Ok(Arc::new(LimitedTools {
            inner: self.create(root, profile)?,
            output_bytes: output_bytes as usize,
            slots: tokio::sync::Semaphore::new(usize::from(concurrency)),
        }))
    }
}
struct LimitedTools {
    inner: Arc<dyn RunTool>,
    output_bytes: usize,
    slots: tokio::sync::Semaphore,
}
#[async_trait::async_trait]
impl RunTool for LimitedTools {
    fn executable_tools(&self) -> Vec<String> {
        self.inner.executable_tools()
    }
    fn parallel_safe(&self, name: &str, args: &serde_json::Value) -> bool {
        self.inner.parallel_safe(name, args)
    }
    fn requires_approval(&self, name: &str, args: &serde_json::Value) -> bool {
        self.inner.requires_approval(name, args)
    }
    async fn execute(
        &self,
        request: ait_ports::ToolInvocation,
    ) -> Result<ait_ports::ToolOutcome, DomainError> {
        let permit=tokio::select!{
            ()=request.cancellation.cancelled()=>return Err(DomainError::invariant(ErrorCode::RunCancelled,"tool cancelled")),
            permit=self.slots.acquire()=>permit,
        }.map_err(|_|DomainError::invariant(ErrorCode::RunCancelled,"tool executor closed"))?;
        let result = self.inner.execute(request).await?;
        drop(permit);
        if serde_json::to_vec(&result.output).map_or(true, |b| b.len() > self.output_bytes) {
            return Err(DomainError::invariant(
                ErrorCode::RunLimitExceeded,
                "tool output exceeded limit",
            ));
        }
        Ok(result)
    }
    async fn cancel_and_drain(&self) {
        self.slots.close();
        self.inner.cancel_and_drain().await;
    }
    async fn reconcile(
        &self,
        execution: &ait_domain::ToolExecution,
    ) -> Result<ait_ports::ToolRecovery, DomainError> {
        self.inner.reconcile(execution).await
    }
}
impl RunToolFactory for SandboxToolFactory {
    fn create(
        &self,
        root: &Path,
        profile: RunPermissionProfile,
    ) -> Result<Arc<dyn RunTool>, DomainError> {
        if profile.sandbox > self.maximum {
            return Err(DomainError::invariant(
                ErrorCode::ToolApprovalRequired,
                "Run permission exceeds administrator ceiling",
            ));
        }
        // HostToolFactory uses capability-relative, no-symlink handles and
        // bounded controlled commands; full_access does not grant arbitrary shell.
        ait_tools::host::HostToolFactory.create(root, profile)
    }
}

/// Spawn a worker in an owned Unix process group or Windows kill-on-close Job.
/// Credentials, Run data and Project paths are delivered through stdin only.
///
/// # Errors
/// Returns an OS spawn error to the caller, which must publish only a stable code.
pub fn spawn_worker(binary: &Path) -> std::io::Result<Box<dyn ChildWrapper>> {
    let mut command = CommandWrap::with_new(binary, |command| {
        command
            .args(["--stdio", "--protocol-major", "1"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Only non-secret OS discovery variables survive. No provider/Codex
        // credential variables, arbitrary application settings or inherited handles.
        for name in [
            "PATH",
            "HOME",
            "USERPROFILE",
            "SYSTEMROOT",
            "WINDIR",
            "TMPDIR",
            "TEMP",
            "TMP",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    });
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    command.spawn()
}

/// Last-resort parent-EOF cleanup, called only by the dedicated worker binary.
/// On Unix a worker created by our supervisor is its own process-group leader;
/// kill the remaining group before exit, even when the daemon has died.
/// Windows jobs are owned by the daemon and killed automatically on handle close.
pub fn cleanup_worker_group() {
    #[cfg(unix)]
    if nix::unistd::getpgrp() == nix::unistd::getpid() {
        let _ = nix::sys::signal::killpg(nix::unistd::getpgrp(), nix::sys::signal::Signal::SIGKILL);
    }
}
