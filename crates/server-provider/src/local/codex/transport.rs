use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::ports::agent_session::AgentSessionError;

const MAX_FRAME: usize = 2 * 1024 * 1024;
const MAX_EVENTS: usize = 128;

#[derive(Debug)]
pub(super) struct Transport {
    child: Child,
    input: Arc<Mutex<ChildStdin>>,
    messages: mpsc::Receiver<Value>,
    reader: JoinHandle<()>,
    events: VecDeque<Value>,
    sequence: u64,
    deadline: Duration,
    closed: bool,
}

impl Transport {
    pub(super) fn spawn(
        program: &Path,
        cwd: &str,
        deadline: Duration,
    ) -> Result<Self, AgentSessionError> {
        let mut command = Command::new(program);
        command.arg("app-server")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Native diagnostics may contain prompts, paths or authentication data.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| AgentSessionError::Unavailable)?;
        let input = Arc::new(Mutex::new(
            child.stdin.take().ok_or(AgentSessionError::Failed)?,
        ));
        let output = child.stdout.take().ok_or(AgentSessionError::Failed)?;
        let (sender, messages) = mpsc::channel(MAX_EVENTS);
        let request_input = input.clone();
        let reader = tokio::spawn(async move {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let read = (&mut output)
                    .take(MAX_FRAME as u64)
                    .read_until(b'\n', &mut bytes)
                    .await;
                if !matches!(read, Ok(1..)) || bytes.last() != Some(&b'\n') {
                    break;
                }
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    break;
                };
                if value.get("method").is_some() && value.get("id").is_some() {
                    // Permissions and user-input interactions are not implemented. Never approve
                    // them implicitly or leave a native turn waiting indefinitely.
                    let rejection = json!({"id":value["id"],"error":{
                        "code":-32601,"message":"Provider interaction is not supported"}});
                    let _ = tokio::time::timeout(deadline, write(&request_input, &rejection)).await;
                    let _ = sender
                        .send(json!({"method":"server/unsupportedRequest"}))
                        .await;
                    break;
                }
                // This phase publishes terminal text, not token or tool streams. Discard their
                // intermediate notifications here instead of retaining an unbounded transcript.
                if value.get("id").is_none()
                    && value["method"] != "turn/completed"
                    && !(value["method"] == "item/completed"
                        && value["params"]["item"]["type"] == "agentMessage")
                {
                    continue;
                }
                if sender.send(value).await.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            messages,
            reader,
            events: VecDeque::new(),
            sequence: 0,
            deadline,
            closed: false,
        })
    }

    pub(super) async fn initialize(&mut self) -> Result<(), AgentSessionError> {
        self.request(
            "initialize",
            json!({
                "clientInfo":{"name":"ait-server","version":env!("CARGO_PKG_VERSION")},
                "capabilities":{"experimentalApi":true}
            }),
        )
        .await?;
        let result = tokio::time::timeout(
            self.deadline,
            write(&self.input, &json!({"method":"initialized","params":{}})),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            let _ = self.close().await;
            return Err(AgentSessionError::Failed);
        }
        Ok(())
    }

    pub(super) async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, AgentSessionError> {
        if self.closed {
            return Err(AgentSessionError::Failed);
        }
        self.sequence += 1;
        let request = json!({"id":self.sequence,"method":method,"params":params});
        let result = tokio::time::timeout(self.deadline, async {
            write(&self.input, &request).await?;
            loop {
                let message = self
                    .messages
                    .recv()
                    .await
                    .ok_or(AgentSessionError::Failed)?;
                if let Some(id) = message.get("id") {
                    if id.as_u64() != Some(self.sequence) {
                        return Err(AgentSessionError::Failed);
                    }
                    if message.get("error").is_some() {
                        return Err(AgentSessionError::Rejected);
                    }
                    return message
                        .get("result")
                        .cloned()
                        .ok_or(AgentSessionError::Failed);
                }
                if self.events.len() >= MAX_EVENTS {
                    return Err(AgentSessionError::Failed);
                }
                self.events.push_back(message);
            }
        })
        .await;
        if let Ok(Ok(value)) = result {
            Ok(value)
        } else if matches!(result, Ok(Err(AgentSessionError::Rejected))) {
            Err(AgentSessionError::Rejected)
        } else {
            let _ = self.close().await;
            Err(AgentSessionError::Failed)
        }
    }

    pub(super) fn poll(&mut self) -> Result<Option<Value>, AgentSessionError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        match self.messages.try_recv() {
            Ok(message) if message.get("id").is_none() => Ok(Some(message)),
            Err(mpsc::error::TryRecvError::Empty) if !self.closed => Ok(None),
            Ok(_) | Err(_) => Err(AgentSessionError::Failed),
        }
    }

    pub(super) async fn close(&mut self) -> Result<(), AgentSessionError> {
        self.closed = true;
        self.reader.abort();
        #[cfg(unix)]
        if let Some(id) = self.child.id() {
            let mut signal = Command::new("/bin/kill");
            signal
                .args(["-KILL", &format!("-{id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let _ = tokio::time::timeout(Duration::from_secs(1), signal.status()).await;
        }
        // kill is idempotent for an already-exited child; wait always reaps it.
        let _ = self.child.start_kill();
        tokio::time::timeout(Duration::from_secs(2), self.child.wait())
            .await
            .map_err(|_| AgentSessionError::Failed)?
            .map_err(|_| AgentSessionError::Failed)?;
        Ok(())
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
        #[cfg(unix)]
        if let Some(id) = self.child.id() {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", &format!("-{id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        // kill_on_drop handles exceptional paths; normal ownership ends through close + wait.
    }
}

async fn write(input: &Mutex<ChildStdin>, value: &Value) -> Result<(), AgentSessionError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| AgentSessionError::Failed)?;
    if bytes.len() >= MAX_FRAME {
        return Err(AgentSessionError::Failed);
    }
    bytes.push(b'\n');
    let mut input = input.lock().await;
    input
        .write_all(&bytes)
        .await
        .map_err(|_| AgentSessionError::Failed)?;
    input.flush().await.map_err(|_| AgentSessionError::Failed)
}
