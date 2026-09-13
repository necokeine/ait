//! Bounded admission; started workers own permits/resources until cancellation drains.
use super::error;
use ait_domain::{DomainError, DomainMetadata, ErrorCode};
use std::{
    ffi::OsStr,
    io::{self, Read, Seek},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: u64 = 1024 * 1024;
const QUEUED: u8 = 0;
const STARTED: u8 = 1;
const CANCELLED: u8 = 2;

/// The async owner can take back unstarted work without waiting for Tokio to
/// dequeue/drop its cancelled task. Once STARTED wins, only the worker owns it.
struct QueuedWorker<T> {
    phase: Arc<AtomicU8>,
    payload: Arc<Mutex<Option<T>>>,
    abort: tokio::task::AbortHandle,
}

impl<T> QueuedWorker<T> {
    fn cancel_queued(&self) -> bool {
        if self
            .phase
            .compare_exchange(QUEUED, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        let payload = self
            .payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.abort.abort();
        // Release captured leases and the capacity permit now, even if Tokio's
        // blocking pool cannot yet dequeue the aborted task. Drop outside the lock.
        drop(payload);
        true
    }
}

impl<T> Drop for QueuedWorker<T> {
    fn drop(&mut self) {
        self.cancel_queued();
    }
}

/// Configuration is shared by adapter clones; every public call starts a new scope.
#[derive(Clone)]
pub(crate) struct OperationOptions {
    pub(crate) timeout: Duration,
    #[cfg(test)]
    pub(crate) probe: Option<Probe>,
    #[cfg(test)]
    pub(crate) elapsed_ms: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    pub(crate) permits: Option<Arc<Semaphore>>,
    #[cfg(test)]
    pub(crate) git_program: Option<PathBuf>,
}

impl Default for OperationOptions {
    fn default() -> Self {
        Self {
            timeout: DEADLINE,
            #[cfg(test)]
            probe: None,
            #[cfg(test)]
            elapsed_ms: Arc::default(),
            #[cfg(test)]
            permits: None,
            #[cfg(test)]
            git_program: None,
        }
    }
}

impl OperationOptions {
    // Instance state is used by deterministic test clock/admission probes.
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn now(&self) -> Instant {
        let now = Instant::now();
        #[cfg(test)]
        let now =
            now + Duration::from_millis(self.elapsed_ms.load(std::sync::atomic::Ordering::SeqCst));
        now
    }

    // Instance state is used by deterministic test clock/admission probes.
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn permits(&self) -> Arc<Semaphore> {
        static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        #[cfg(test)]
        if let Some(permits) = &self.permits {
            return permits.clone();
        }
        PERMITS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
    }
}

/// A single cancellation/deadline owner spanning all queues and blocking phases.
pub(crate) struct Operation {
    context: Arc<BlockingContext>,
    _cancel_on_drop: tokio_util::sync::DropGuard,
}

pub(crate) struct BlockingContext {
    cancellation: CancellationToken,
    deadline: Instant,
    failure_code: ErrorCode,
    options: OperationOptions,
    retained: Mutex<Vec<RetainedPath>>,
    #[cfg(test)]
    after_git: Option<GitObserver>,
}

struct RetainedPath {
    path: PathBuf,
    state: &'static str,
}

#[cfg(test)]
type GitObserver = Box<dyn Fn(&Command) + Send + Sync>;
#[cfg(test)]
pub(crate) type Probe = Arc<dyn Fn(&str, &BlockingContext) + Send + Sync>;

impl BlockingContext {
    pub(crate) fn check(&self) -> Result<(), DomainError> {
        if self.cancellation.is_cancelled() {
            Err(error(
                ErrorCode::RunCancelled,
                "Project operation cancelled",
                false,
            ))
        } else if self.options.now() >= self.deadline {
            Err(self.timeout_error())
        } else {
            Ok(())
        }
    }

    fn timeout_error(&self) -> DomainError {
        error(self.failure_code, "Project operation timed out", true).with_details(DomainMetadata(
            [("reason".into(), serde_json::json!("timeout"))].into(),
        ))
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(self.options.now())
    }

    /// Record intent before a mutation, then replace it with the confirmed outcome.
    /// This survives a later failed verification, deadline, or worker panic.
    pub(crate) fn retain(&self, path: &Path, state: &'static str) {
        let mut retained = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = retained.iter_mut().find(|item| item.path == path) {
            existing.state = state;
        } else {
            retained.push(RetainedPath {
                path: path.to_owned(),
                state,
            });
        }
    }

    pub(crate) fn forget_retained(&self, path: &Path) {
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|item| item.path != path);
    }

    fn report_retained(&self, mut failure: DomainError) -> DomainError {
        let retained = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !retained.is_empty() {
            failure.retryable = false;
            let paths = retained
                .iter()
                .map(|item| format!("{} ({})", item.path.display(), item.state))
                .collect::<Vec<_>>()
                .join(", ");
            failure.message = format!(
                "{}; retained filesystem state at {paths}. Inspect it before retrying.",
                failure.message
            );
            failure
                .details
                .get_or_insert_with(DomainMetadata::default)
                .0
                .insert(
                    "retained_paths".into(),
                    serde_json::json!(
                        retained
                            .iter()
                            .map(|item| serde_json::json!({"path": item.path, "state": item.state}))
                            .collect::<Vec<_>>()
                    ),
                );
        }
        failure
    }

    // Instance state is used by deterministic test clock/admission probes.
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    pub(crate) fn point(&self, name: &str) {
        #[cfg(test)]
        if let Some(probe) = &self.options.probe {
            probe(name, self);
        }
        #[cfg(not(test))]
        let _ = name;
    }

    pub(super) fn command(&self) -> GitCommand<'_> {
        let program = Path::new("git");
        #[cfg(test)]
        let program = self.options.git_program.as_deref().unwrap_or(program);
        let mut command = Command::new(program);
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null());
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

impl Operation {
    pub(crate) fn new(options: &OperationOptions, failure_code: ErrorCode) -> Self {
        let cancellation = CancellationToken::new();
        Self {
            _cancel_on_drop: cancellation.clone().drop_guard(),
            context: Arc::new(BlockingContext {
                deadline: options.now() + options.timeout,
                cancellation,
                failure_code,
                options: options.clone(),
                retained: Mutex::default(),
                #[cfg(test)]
                after_git: None,
            }),
        }
    }

    pub(crate) fn context(&self) -> &BlockingContext {
        &self.context
    }

    /// Queues consume the same remaining budget, never a freshly allocated timeout.
    pub(crate) async fn wait<T>(
        &self,
        future: impl std::future::Future<Output = T>,
    ) -> Result<T, DomainError> {
        self.context
            .check()
            .map_err(|error| self.context.report_retained(error))?;
        let result = tokio::time::timeout(self.context.remaining(), future)
            .await
            .map_err(|_| self.context.report_retained(self.context.timeout_error()))?;
        self.context
            .check()
            .map_err(|error| self.context.report_retained(error))?;
        Ok(result)
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&BlockingContext) -> Result<T, DomainError> + Send + 'static,
    ) -> Result<T, DomainError> {
        let permit = self
            .wait(self.context.options.permits().acquire_owned())
            .await?
            .map_err(|_| {
                error(
                    self.context.failure_code,
                    "Project I/O capacity unavailable",
                    true,
                )
            })?;
        let context = self.context.clone();
        let phase = Arc::new(AtomicU8::new(QUEUED));
        let worker_phase = phase.clone();
        let payload = Arc::new(Mutex::new(Some((permit, operation))));
        let worker_payload = payload.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            if worker_phase
                .compare_exchange(QUEUED, STARTED, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err(context.timeout_error());
            }
            let (permit, operation) = worker_payload
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("STARTED worker exclusively owns its queued payload");
            let _permit = permit;
            context.point("before_blocking");
            context.check()?;
            let result = operation(&context);
            // A late mutation may still have succeeded: its recorded path/state
            // accompanies this error, even when no normal return value is delivered.
            context.check()?;
            result
        });
        let queued_worker = QueuedWorker {
            phase,
            payload,
            abort: worker.abort_handle(),
        };
        self.context.point("worker_queued");
        // Tokio has its own blocking queue. Abort a still-queued job at the same
        // deadline; an already-started syscall must drain before reporting its
        // retained state and releasing resources.
        let result =
            if let Ok(joined) = tokio::time::timeout(self.context.remaining(), &mut worker).await {
                joined.unwrap_or_else(|_| {
                    Err(error(
                        self.context.failure_code,
                        "Project I/O worker failed",
                        false,
                    ))
                })
            } else {
                // The same RAII cancellation path handles deadline and future
                // drop. Started work still drains before reporting retained state.
                if !queued_worker.cancel_queued() {
                    let _ = worker.await;
                }
                Err(self.context.timeout_error())
            };
        result.map_err(|error| self.context.report_retained(error))
    }
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
