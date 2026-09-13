//! Bounded admission; started workers own permits/resources until cancellation drains.
use super::error;
use ait_domain::{DomainError, ErrorCode};
use std::{
    ffi::OsStr,
    io::{self, Read, Seek},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: u64 = 1024 * 1024;

pub(crate) struct BlockingContext {
    cancellation: CancellationToken,
    deadline: Instant,
    #[cfg(test)]
    after_git: Option<GitObserver>,
}

#[cfg(test)]
type GitObserver = Box<dyn Fn(&Command) + Send + Sync>;

impl BlockingContext {
    pub(super) fn check(&self) -> Result<(), DomainError> {
        if self.cancellation.is_cancelled() {
            Err(error(
                ErrorCode::RunCancelled,
                "Project operation cancelled; partial filesystem work is retained",
                false,
            ))
        } else if Instant::now() >= self.deadline {
            Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                "Project operation timed out; inspect retained filesystem state before retrying",
                true,
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn command(&self) -> GitCommand<'_> {
        let mut command = Command::new("git");
        command.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
        // No manager operation needs user hooks or a filesystem monitor process.
        command.args(["-c", "core.hooksPath=", "-c", "core.fsmonitor=false"]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        GitCommand {
            context: self,
            command,
        }
    }
}

pub(crate) async fn run<T: Send + 'static>(
    operation: impl FnOnce(&BlockingContext) -> Result<T, DomainError> + Send + 'static,
) -> Result<T, DomainError> {
    static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let deadline = Instant::now() + DEADLINE;
    let permit = tokio::time::timeout(
        DEADLINE,
        PERMITS
            .get_or_init(|| Arc::new(Semaphore::new(4)))
            .clone()
            .acquire_owned(),
    )
    .await
    .map_err(|_| {
        error(
            ErrorCode::ProjectWorkspaceBusy,
            "Project I/O capacity timed out",
            true,
        )
    })?
    .map_err(|_| {
        error(
            ErrorCode::ProjectWorkspaceBusy,
            "Project I/O capacity unavailable",
            true,
        )
    })?;
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let context = BlockingContext {
            cancellation,
            deadline,
            #[cfg(test)]
            after_git: None,
        };
        context.check()?;
        let result = operation(&context);
        context.check()?;
        result
    })
    .await
    .map_err(|_| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Project I/O worker failed",
            false,
        )
    })?
}

pub(super) struct GitCommand<'a> {
    context: &'a BlockingContext,
    command: Command,
}
impl GitCommand<'_> {
    pub(super) fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.command.arg(arg);
        self
    }
    pub(super) fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command.args(args);
        self
    }
    pub(super) fn output(&mut self) -> io::Result<Output> {
        self.context.check().map_err(io::Error::other)?;
        // File-backed capture avoids pipe deadlock and reader-thread leaks. Poll
        // size while Git runs and cap reads even if the process exits between polls.
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        self.command
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?);
        let mut child = ChildGuard(self.command.spawn()?);
        let status = loop {
            self.context.check().map_err(io::Error::other)?;
            if stdout.metadata()?.len() > OUTPUT_LIMIT || stderr.metadata()?.len() > OUTPUT_LIMIT {
                return Err(io::Error::other("Git output exceeds the Project I/O limit"));
            }
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        stdout.rewind()?;
        stderr.rewind()?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        stdout.take(OUTPUT_LIMIT + 1).read_to_end(&mut out)?;
        stderr.take(OUTPUT_LIMIT + 1).read_to_end(&mut err)?;
        if out.len() as u64 > OUTPUT_LIMIT || err.len() as u64 > OUTPUT_LIMIT {
            return Err(io::Error::other("Git output exceeds the Project I/O limit"));
        }
        #[cfg(test)]
        if let Some(observe) = &self.context.after_git {
            observe(&self.command);
        }
        Ok(Output {
            status,
            stdout: out,
            stderr: err,
        })
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Reap before releasing the worker permit and any borrowed workspace lease.
        if self.0.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            {
                if let Ok(pid) = i32::try_from(self.0.id()) {
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod tests;
