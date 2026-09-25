//! Bounded transport-neutral output queue.

use std::sync::Arc;

use crate::ServerMessage;
use crate::server::{MAX_QUEUE_BYTES, MAX_QUEUE_MESSAGES};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
/// Failure to encode or enqueue outgoing connection data.
pub enum QueueError {
    /// The JSON payload could not be serialized.
    #[error("outgoing message cannot be encoded")]
    Encode(#[from] serde_json::Error),
    /// The queue is full or the connection has closed.
    #[error("outgoing connection budget exhausted or connection closed")]
    Full,
}

#[derive(Debug)]
/// One queued frame that retains its byte budget through the active socket write.
pub struct Queued {
    /// Encoded frame to write.
    pub message: Frame,
    // Budget remains reserved until the active socket write completes or is dropped.
    _bytes: OwnedSemaphorePermit,
}

#[derive(Clone, Debug)]
/// Cloneable bounded connection output.
pub struct Outbound {
    sender: mpsc::Sender<Queued>,
    bytes: Arc<Semaphore>,
    failed: CancellationToken,
}

impl Outbound {
    /// Create an output queue and its single transport receiver.
    #[must_use]
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

    /// Enqueue an encoded server message without waiting.
    /// # Errors
    /// Cancels the failure token if encoding or a queue budget fails.
    pub fn send(&self, message: &ServerMessage) -> Result<(), QueueError> {
        let result = self.try_send(message);
        if result.is_err() {
            self.failed.cancel();
        }
        result
    }

    /// Return the token cancelled when delivery fails or the connection ends.
    #[must_use]
    pub fn failure(&self) -> CancellationToken {
        self.failed.clone()
    }

    /// Send one correlated response or stable error envelope.
    /// # Errors
    /// Returns an encoding or queue failure.
    pub fn respond(
        &self,
        request_id: String,
        result: Result<serde_json::Value, crate::ErrorCode>,
    ) -> Result<(), QueueError> {
        match result {
            Ok(result) => self.send(&ServerMessage::Response { request_id, result }),
            Err(code) => self.send(&ServerMessage::Error {
                request_id: Some(request_id),
                code,
                message: code.message().to_owned(),
                retryable: code.retryable(),
            }),
        }
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
                message: Frame::Text(text),
                _bytes: bytes,
            })
            .map_err(|_| QueueError::Full)
    }

    /// Queue binary data, waiting within the connection byte budget.
    /// # Errors
    /// Rejects oversized data and interrupted or closed delivery.
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
            result = self.sender.send(Queued { message: Frame::Binary(data), _bytes: bytes }) => result.map_err(|_| QueueError::Full),
        }
    }
}

#[cfg(test)]
mod tests;

/// Encoded connection data; the API converts this to a WebSocket frame.
#[derive(Debug, Clone)]
pub enum Frame {
    /// Serialized JSON response or event.
    Text(String),
    /// Encoded binary protocol frame.
    Binary(Vec<u8>),
}
