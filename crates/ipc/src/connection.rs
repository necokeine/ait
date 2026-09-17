//! Owned, bounded pipe pumps; no partially read frame is cancelled by select.
use crate::codec::{Reader, Writer};
use ait_contracts::worker::{Envelope, Lease, Payload, ProtocolError};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

type Outgoing = (Payload, oneshot::Sender<Result<(), ProtocolError>>);
/// A connection owns and joins both I/O pumps.
pub struct Connection {
    sender: mpsc::Sender<Outgoing>,
    /// Bounded incoming queue. EOF is delivered as an error once.
    pub receiver: mpsc::Receiver<Result<Envelope, ProtocolError>>,
    tasks: Vec<JoinHandle<()>>,
}
impl Connection {
    /// Start pumps after the handshake, preserving framing sequence numbers.
    pub fn start<R, W>(
        mut reader: Reader<R>,
        mut writer: Writer<W>,
        lease: Lease,
        secret: Option<String>,
    ) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (incoming, receiver) = mpsc::channel(16);
        let (sender, mut outgoing) = mpsc::channel::<Outgoing>(16);
        let incoming_secret = secret.clone();
        let read = tokio::spawn(async move {
            loop {
                let result = reader.read().await.and_then(|frame| {
                    if leaks(&frame.payload, incoming_secret.as_deref()) {
                        Err(ProtocolError::InvalidFrame)
                    } else {
                        Ok(frame)
                    }
                });
                let failed = result.is_err();
                if incoming.send(result).await.is_err() || failed {
                    break;
                }
            }
        });
        let write = tokio::spawn(async move {
            while let Some((payload, ack)) = outgoing.recv().await {
                // Reject credential echoes before serialization reaches the pipe.
                let leak = leaks(&payload, secret.as_deref());
                let result = if leak {
                    Err(ProtocolError::InvalidFrame)
                } else {
                    tokio::time::timeout(
                        Duration::from_secs(1),
                        writer.write(Some(lease.clone()), payload),
                    )
                    .await
                    .unwrap_or(Err(ProtocolError::Io))
                };
                let failed = result.is_err();
                let _ = ack.send(result);
                if failed {
                    break;
                }
            }
        });
        Self {
            sender,
            receiver,
            tasks: vec![read, write],
        }
    }
    /// Send and wait until bytes are flushed. No unbounded slow-consumer wait.
    /// # Errors
    /// Reports a closed, polluted or stalled pipe with a stable code.
    pub async fn send(&self, payload: Payload) -> Result<(), ProtocolError> {
        let (ack, flushed) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(2), async {
            self.sender
                .send((payload, ack))
                .await
                .map_err(|_| ProtocolError::Io)?;
            flushed.await.map_err(|_| ProtocolError::Io)?
        })
        .await
        .unwrap_or(Err(ProtocolError::Io))
    }
    /// Abort and join pumps before releasing a process handle.
    pub async fn close(mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }
}

// Inspect decoded strings, including object keys: JSON escaping must not bypass
// the private-grant boundary in either direction of the untrusted worker pipe.
fn leaks(payload: &Payload, secret: Option<&str>) -> bool {
    fn contains(value: &serde_json::Value, secret: &str) -> bool {
        match value {
            serde_json::Value::String(text) => text.contains(secret),
            serde_json::Value::Array(values) => values.iter().any(|v| contains(v, secret)),
            serde_json::Value::Object(values) => values
                .iter()
                .any(|(key, value)| key.contains(secret) || contains(value, secret)),
            _ => false,
        }
    }
    secret.filter(|s| !s.is_empty()).is_some_and(|secret| {
        serde_json::to_value(payload).map_or(true, |value| contains(&value, secret))
    })
}
impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests;
