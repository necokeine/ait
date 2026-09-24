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
    pub message: axum::extract::ws::Message,
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
                message: axum::extract::ws::Message::Text(text.into()),
                _bytes: bytes,
            })
            .map_err(|_| QueueError::Full)
    }

    pub async fn binary(&self, data: Vec<u8>) -> Result<(), QueueError> {
        let count = u32::try_from(data.len()).map_err(|_| QueueError::Full)?;
        if data.len() > MAX_QUEUE_BYTES {
            return Err(QueueError::Full);
        }
        let bytes = tokio::select! {
            () = self.failed.cancelled() => return Err(QueueError::Full),
            permit = self.bytes.clone().acquire_many_owned(count) => permit.map_err(|_| QueueError::Full)?,
        };
        tokio::select! {
            () = self.failed.cancelled() => Err(QueueError::Full),
            result = self.sender.send(Queued { message: axum::extract::ws::Message::Binary(data.into()), _bytes: bytes }) => result.map_err(|_| QueueError::Full),
        }
    }
}

#[cfg(test)]
mod tests;

impl From<server_metadata::rpc::workspace_labels::DeliveryError> for QueueError {
    fn from(error: server_metadata::rpc::workspace_labels::DeliveryError) -> Self {
        match error {
            server_metadata::rpc::workspace_labels::DeliveryError::Encode(error) => {
                Self::Encode(error)
            }
            server_metadata::rpc::workspace_labels::DeliveryError::Closed => Self::Full,
        }
    }
}
