//! Host execution and durable storage boundaries.
use crate::protocol::Schedule;
use std::{future::Future, pin::Pin};
use tokio_util::sync::CancellationToken;

/// Stable errors that do not expose provider credentials or storage paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Invalid cadence, target or payload.
    #[error("Invalid schedule parameters")]
    Invalid,
    /// Unknown schedule ID.
    #[error("Schedule not found")]
    NotFound,
    /// Already running, completed or storage capacity reached.
    #[error("Schedule is busy, completed or capacity is exhausted")]
    Conflict,
    /// Atomic persistence or recovery failed.
    #[error("Schedule storage failed")]
    Storage,
}

/// Durable full-state replacement; failed writes must preserve the prior document.
pub trait Store: Send + std::fmt::Debug {
    /// Load durable schedules. Missing storage is an empty list.
    /// # Errors
    /// Returns storage errors for unreadable or invalid documents.
    fn load(&self) -> Result<Vec<Schedule>, Error>;
    /// Atomically replace storage with validated schedules.
    /// # Errors
    /// Returns storage errors without publishing a partial document.
    fn save(&mut self, schedules: &[Schedule]) -> Result<(), Error>;
}

/// Completed execution, including identities allocated before an error.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Agent that ran.
    pub agent_id: Option<String>,
    /// Workspace created for this occurrence.
    pub workspace_id: Option<String>,
    /// Final output.
    pub output: Option<String>,
    /// Safe failure message; absence means success.
    pub error: Option<String>,
    /// Permanently missing target completes the schedule.
    pub target_gone: bool,
}

/// Asynchronous host execution, isolated from persistence and transport.
pub trait Runner: Send + Sync + std::fmt::Debug {
    /// Execute an occurrence; honor cancellation and release owned resources before returning.
    fn run(
        &self,
        schedule: Schedule,
        run_id: String,
        progress: Progress,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>>;
}

/// Acknowledged durable occurrence identity updates, sent before starting the native turn.
#[derive(Debug, Clone)]
pub struct Progress {
    pub(crate) sender: tokio::sync::mpsc::Sender<Checkpoint>,
    pub(crate) schedule_id: String,
    pub(crate) run_id: String,
}
#[derive(Debug)]
pub(crate) struct Checkpoint {
    pub schedule_id: String,
    pub run_id: String,
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub reply: tokio::sync::oneshot::Sender<Result<(), Error>>,
}
impl Progress {
    /// Persist allocated identities before further side effects.
    /// # Errors
    /// Returns storage errors; the runner must clean up its allocated resources on failure.
    pub async fn record(
        &self,
        agent_id: Option<String>,
        workspace_id: Option<String>,
    ) -> Result<(), Error> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.sender
            .send(Checkpoint {
                schedule_id: self.schedule_id.clone(),
                run_id: self.run_id.clone(),
                agent_id,
                workspace_id,
                reply,
            })
            .await
            .map_err(|_| Error::Storage)?;
        receive.await.map_err(|_| Error::Storage)?
    }
}
