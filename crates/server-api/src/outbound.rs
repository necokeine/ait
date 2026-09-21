use std::sync::Arc;

use server_protocol::{MAX_QUEUE_BYTES, MAX_QUEUE_MESSAGES, ServerMessage};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

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

pub(super) struct Outbound {
    sender: mpsc::Sender<Queued>,
    bytes: Arc<Semaphore>,
}

impl Outbound {
    pub fn new() -> (Self, mpsc::Receiver<Queued>) {
        let (sender, receiver) = mpsc::channel(MAX_QUEUE_MESSAGES);
        (
            Self {
                sender,
                bytes: Arc::new(Semaphore::new(MAX_QUEUE_BYTES)),
            },
            receiver,
        )
    }

    pub fn send(&self, message: &ServerMessage) -> Result<(), QueueError> {
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
