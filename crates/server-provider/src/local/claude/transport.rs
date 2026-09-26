use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use super::ClaudeClient;
use crate::ports::agent_session::{AgentSessionError, AgentSessionSpec};

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
        client: &ClaudeClient,
        spec: &AgentSessionSpec,
        binding: Option<(&str, bool)>,
        output_schema: Option<&Value>,
    ) -> Result<Self, AgentSessionError> {
        #[cfg(windows)]
        if client
            .program
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            })
        {
            return Err(AgentSessionError::Unavailable);
        }
        let mut command = launch_command(client, spec, binding)?;
        if let Some(schema) = output_schema {
            command.arg(format!("--json-schema={schema}"));
        }
        let mut child = command
            .spawn()
            .map_err(|_| AgentSessionError::Unavailable)?;
        let input = Arc::new(Mutex::new(
            child.stdin.take().ok_or(AgentSessionError::Failed)?,
        ));
        let output = child.stdout.take().ok_or(AgentSessionError::Failed)?;
        let (sender, messages) = mpsc::channel(MAX_EVENTS);
        let reader_input = input.clone();
        let deadline = client.deadline;
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
                if bytes.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
                    break;
                };
                if message["type"] == "control_request"
                    && message["request"]["subtype"] != "can_use_tool"
                {
                    let reply = json!({"type":"control_response","response":{
                        "subtype":"error","request_id":message["request_id"],"error":"Unsupported provider interaction"}});
                    let _ = tokio::time::timeout(deadline, write(&reader_input, &reply)).await;
                    break;
                }
                if sender.send(message).await.is_err() {
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

    pub(super) async fn initialize(&mut self) -> Result<Value, AgentSessionError> {
        self.request(json!({"subtype":"initialize","hooks":{}}))
            .await
    }

    pub(super) async fn request(&mut self, request: Value) -> Result<Value, AgentSessionError> {
        if self.closed {
            return Err(AgentSessionError::Failed);
        }
        self.sequence += 1;
        let id = format!("ait-{}", self.sequence);
        let result = tokio::time::timeout(self.deadline, async {
            write(
                &self.input,
                &json!({"type":"control_request","request_id":id,"request":request}),
            )
            .await?;
            loop {
                let message = self
                    .messages
                    .recv()
                    .await
                    .ok_or(AgentSessionError::Failed)?;
                if message["type"] == "control_response" {
                    let response = &message["response"];
                    if response["request_id"] != id {
                        return Err(AgentSessionError::Failed);
                    }
                    return match response["subtype"].as_str() {
                        Some("success") => Ok(response
                            .get("response")
                            .cloned()
                            .unwrap_or_else(|| json!({}))),
                        Some("error") => Err(AgentSessionError::Rejected),
                        _ => Err(AgentSessionError::Failed),
                    };
                }
                if self.events.len() >= MAX_EVENTS {
                    return Err(AgentSessionError::Failed);
                }
                self.events.push_back(message);
            }
        })
        .await;
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(AgentSessionError::Rejected)) => Err(AgentSessionError::Rejected),
            _ => {
                let _ = self.close().await;
                Err(AgentSessionError::Failed)
            }
        }
    }

    pub(super) async fn send(&mut self, message: &Value) -> Result<(), AgentSessionError> {
        if !self.closed
            && matches!(
                tokio::time::timeout(self.deadline, write(&self.input, message)).await,
                Ok(Ok(()))
            )
        {
            return Ok(());
        }
        let _ = self.close().await;
        Err(AgentSessionError::Failed)
    }

    pub(super) fn poll(&mut self) -> Result<Option<Value>, AgentSessionError> {
        if let Some(message) = self.events.pop_front() {
            return Ok(Some(message));
        }
        match self.messages.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::error::TryRecvError::Empty) if !self.closed => Ok(None),
            Err(_) => Err(AgentSessionError::Failed),
        }
    }

    pub(super) async fn close(&mut self) -> Result<(), AgentSessionError> {
        self.closed = true;
        self.reader.abort();
        #[cfg(unix)]
        if let Some(id) = self.child.id() {
            let mut signal = Command::new("/bin/kill");
            signal
                .args(["-KILL", "--", &format!("-{id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let _ = tokio::time::timeout(Duration::from_secs(1), signal.status()).await;
        }
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
                .args(["-KILL", "--", &format!("-{id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

async fn write(input: &Mutex<ChildStdin>, value: &Value) -> Result<(), AgentSessionError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| AgentSessionError::Failed)?;
    if bytes.len() >= MAX_FRAME {
        return Err(AgentSessionError::Rejected);
    }
    bytes.push(b'\n');
    let mut input = input.lock().await;
    input
        .write_all(&bytes)
        .await
        .map_err(|_| AgentSessionError::Failed)?;
    input.flush().await.map_err(|_| AgentSessionError::Failed)
}

fn launch_command(
    client: &ClaudeClient,
    spec: &AgentSessionSpec,
    binding: Option<(&str, bool)>,
) -> Result<Command, AgentSessionError> {
    let mut command = Command::new(&client.program);
    command
        .args([
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            "--permission-prompt-tool",
            "stdio",
            "--setting-sources=user,project,local",
        ])
        .arg(format!(
            "--permission-mode={}",
            spec.config.mode_id.as_deref().unwrap_or("default")
        ))
        .current_dir(&spec.cwd)
        .env_remove("CLAUDECODE")
        .env("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command.args(crate::local::configuration::claude(&spec.config)?);
    if let Some(path) = &client.config_dir {
        command.env("CLAUDE_CONFIG_DIR", path);
    }
    if let Some((id, resume)) = binding {
        command.arg(format!(
            "--{}={id}",
            if resume { "resume" } else { "session-id" }
        ));
    } else {
        command.arg("--no-session-persistence");
    }
    if let Some(model) = &spec.config.model {
        command.arg(format!("--model={model}"));
    }
    if let Some(effort) = &spec.config.thinking_option_id {
        match effort.as_str() {
            "off" => {
                command.arg("--thinking=disabled");
            }
            "ultracode" => {
                command.args(["--thinking=adaptive", "--effort=xhigh"]);
            }
            _ => {
                command.arg(format!("--effort={effort}"));
            }
        }
    }
    if let Some(prompt) = &spec.config.system_prompt {
        command.arg(format!("--append-system-prompt={prompt}"));
    }
    if spec.config.mode_id.as_deref() == Some("bypassPermissions") {
        command.arg("--allow-dangerously-skip-permissions");
    }
    #[cfg(unix)]
    command.process_group(0);
    Ok(command)
}
