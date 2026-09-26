//! Shared admission, cancellation and tracked blocking jobs.

use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{ErrorCode, Lifecycle, ServerInfo};

/// Process lifecycle action executed after request admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleIntent {
    /// Stop the standalone server process.
    Shutdown,
    /// Rebuild the standalone server in-process.
    Restart {
        /// Normalized diagnostic reason.
        reason: String,
    },
}

/// Runtime resources shared by every capability in one server instance.
#[derive(Debug)]
pub struct Runtime {
    /// Public startup identity and installed capabilities.
    pub info: ServerInfo,
    /// Stop accepting new work and cancel background observers.
    pub cancellation: CancellationToken,
    /// Tracks admitted work until shutdown drains it.
    pub tasks: TaskTracker,
    /// Serializes admission with closing the tracker.
    pub admission: Mutex<()>,
    /// First accepted process lifecycle request.
    pub lifecycle_intent: Mutex<Option<LifecycleIntent>>,
    /// Shared short catalog/project job budget.
    pub jobs: Arc<Semaphore>,
    /// Independent terminal I/O budget.
    pub terminal_jobs: Arc<Semaphore>,
    /// Serializes background checkout reads without consuming foreground admission.
    pub checkout_poll_jobs: Arc<Semaphore>,
    /// Bounded Agent completion waits.
    pub execution_waits: Arc<Semaphore>,
}

impl Runtime {
    /// Create runtime resources with the existing server-wide concurrency limits.
    #[must_use]
    pub fn new(info: ServerInfo) -> Self {
        Self {
            info,
            cancellation: CancellationToken::new(),
            tasks: TaskTracker::new(),
            admission: Mutex::new(()),
            lifecycle_intent: Mutex::new(None),
            jobs: Arc::new(Semaphore::new(1)),
            terminal_jobs: Arc::new(Semaphore::new(4)),
            checkout_poll_jobs: Arc::new(Semaphore::new(1)),
            execution_waits: Arc::new(Semaphore::new(32)),
        }
    }

    /// Return public metadata with the current admission state.
    #[must_use]
    pub fn info(&self) -> ServerInfo {
        let mut info = self.info.clone();
        if self.cancellation.is_cancelled() {
            info.lifecycle = Lifecycle::Draining;
        }
        info
    }

    /// Run `execute` against an installed service outside the Tokio reactor.
    ///
    /// Admission, the job permit and task tracking survive a dropped response future.
    /// # Errors
    /// Rejects missing services, draining or exhausted runtime resources. A poisoned service
    /// lock or panicked blocking task returns `failure`; business errors are passed through.
    pub async fn run<S: Send + 'static, R: Send + 'static>(
        &self,
        service: Option<Arc<Mutex<S>>>,
        failure: ErrorCode,
        execute: impl FnOnce(&mut S) -> Result<R, ErrorCode> + Send + 'static,
    ) -> Result<R, ErrorCode> {
        let service = service.ok_or(ErrorCode::UnsupportedCapability)?;
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.cancellation.is_cancelled() {
            return Err(ErrorCode::ServerDraining);
        }
        let permit = self
            .jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| ErrorCode::ResourceExhausted)?;
        let tracking = self.tasks.token();
        let job = tokio::task::spawn_blocking(move || {
            let (_tracking, _permit) = (tracking, permit);
            let mut service = service.lock().map_err(|_| failure)?;
            execute(&mut service)
        });
        drop(admission);
        job.await.map_err(|_| failure)?
    }
}

#[cfg(test)]
mod tests;
