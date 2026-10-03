//! Connection-owned periodic observations with cancellation-safe delivery.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::outbound::Outbound;
use crate::{ErrorCode, Runtime, ServerMessage};

/// RAII ownership of a tracked observation task and its delivery gate.
#[derive(Debug)]
pub struct Subscription {
    cancellation: CancellationToken,
    active: Arc<Mutex<bool>>,
}

impl Subscription {
    /// Start periodic reads after the caller has admitted the initial response.
    ///
    /// `read` supplies capability-owned events and advances its checkpoint only on success.
    /// Foreground admission exhaustion retries on the next tick; other read failures close the
    /// connection so a client cannot retain an apparently healthy but stale subscription.
    #[must_use]
    pub fn spawn<F, R>(runtime: Arc<Runtime>, outbound: Outbound, read: F) -> Self
    where
        F: FnMut() -> R + Send + 'static,
        R: Future<Output = Result<Vec<ServerMessage>, ErrorCode>> + Send,
    {
        Self::spawn_inner(runtime, outbound, None, read)
    }

    /// Read when committed state changes, with a slow audit for missed external changes.
    #[must_use]
    pub fn spawn_on_changes<F, R>(
        runtime: Arc<Runtime>,
        outbound: Outbound,
        changes: watch::Receiver<()>,
        read: F,
    ) -> Self
    where
        F: FnMut() -> R + Send + 'static,
        R: Future<Output = Result<Vec<ServerMessage>, ErrorCode>> + Send,
    {
        Self::spawn_inner(runtime, outbound, Some(changes), read)
    }

    fn spawn_inner<F, R>(
        runtime: Arc<Runtime>,
        outbound: Outbound,
        mut changes: Option<watch::Receiver<()>>,
        mut read: F,
    ) -> Self
    where
        F: FnMut() -> R + Send + 'static,
        R: Future<Output = Result<Vec<ServerMessage>, ErrorCode>> + Send,
    {
        let cancellation = CancellationToken::new();
        let task_cancel = cancellation.clone();
        let active = Arc::new(Mutex::new(true));
        let task_active = active.clone();
        let admission_runtime = runtime.clone();
        let admission = admission_runtime
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime.cancellation.is_cancelled() {
            cancellation.cancel();
            return Self {
                cancellation,
                active,
            };
        }
        let tracker = runtime.tasks.clone();
        tracker.spawn(async move {
            let period = if changes.is_some() {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(250)
            };
            let mut interval = tokio::time::interval_at(Instant::now() + period, period);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            if let Some(changes) = &mut changes {
                changes.mark_changed();
            }
            let failure = outbound.failure();
            loop {
                tokio::select! {
                    biased;
                    () = task_cancel.cancelled() => break,
                    () = runtime.cancellation.cancelled() => break,
                    () = failure.cancelled() => break,
                    _ = interval.tick() => {},
                    () = async {
                        match &mut changes {
                            Some(changes) => { let _ = changes.changed().await; }
                            None => std::future::pending().await,
                        }
                    } => {},
                }
                let result = tokio::select! {
                    biased;
                    () = task_cancel.cancelled() => break,
                    () = runtime.cancellation.cancelled() => break,
                    () = failure.cancelled() => break,
                    result = read() => result,
                };
                let events = match result {
                    Ok(events) => events,
                    Err(ErrorCode::ResourceExhausted) => continue,
                    Err(_) => {
                        failure.cancel();
                        break;
                    }
                };
                // Release and delivery share this gate, including reads already in flight.
                let active = task_active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !*active {
                    break;
                }
                if events.iter().any(|event| outbound.send(event).is_err()) {
                    failure.cancel();
                    break;
                }
            }
        });
        drop(admission);
        Self {
            cancellation,
            active,
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests;
