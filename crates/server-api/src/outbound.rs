use std::sync::Arc;

use server_protocol::{MAX_QUEUE_BYTES, MAX_QUEUE_MESSAGES, ServerMessage};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub(super) enum QueueError {
    #[error("outgoing message cannot be encoded")]
    Encode(#[from] serde_json::Error),
    #[error("outgoing connection budget exhausted or connection closed")]
    Full,
}

pub(super) struct Queued {
    pub text: String,
    // Budget remains reserved until the active socket write completes or is dropped.
    _bytes: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub(super) struct Outbound {
    sender: mpsc::Sender<Queued>,
    bytes: Arc<Semaphore>,
    failed: CancellationToken,
}

impl Outbound {
    pub fn new() -> (Self, mpsc::Receiver<Queued>) {
        let (sender, receiver) = mpsc::channel(MAX_QUEUE_MESSAGES);
        (
            Self {
                sender,
                bytes: Arc::new(Semaphore::new(MAX_QUEUE_BYTES)),
                failed: CancellationToken::new(),
            },
            receiver,
        )
    }

    pub fn send(&self, message: &ServerMessage) -> Result<(), QueueError> {
        let result = self.try_send(message);
        if result.is_err() {
            self.failed.cancel();
        }
        result
    }

    pub fn failure(&self) -> CancellationToken {
        self.failed.clone()
    }

    fn try_send(&self, message: &ServerMessage) -> Result<(), QueueError> {
        let text = serde_json::to_string(message)?;
        let count = u32::try_from(text.len()).map_err(|_| QueueError::Full)?;
        let bytes = self
            .bytes
            .clone()
            .try_acquire_many_owned(count)
            .map_err(|_| QueueError::Full)?;
        self.sender
            .try_send(Queued {
                text,
                _bytes: bytes,
            })
            .map_err(|_| QueueError::Full)
    }
}

#[cfg(test)]
mod tests;
