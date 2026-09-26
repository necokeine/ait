//! Dedicated native Provider owner. Blocking registry operations stay off HTTP/WS reactors.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use server_metadata::service::session::SessionEvents;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

mod voice;

use crate::ports::agent_runtime::AgentRuntimeRegistry;
use crate::protocol::agent_execution::WaitRequest;
use crate::rpc::ErrorCode;
use crate::rpc::agent_execution::{ExecutionState, only};
use crate::service::agent_manager::AgentManager;
use crate::service::agent_runtime::AgentRuntimeDirectory;

/// Independently composed dependencies retained for the full worker lifetime.
pub struct ExecutionDependencies {
    /// Native session owner with registered clients.
    pub manager: AgentManager,
    /// Agent metadata service using the same registry instance.
    pub directory: AgentRuntimeDirectory,
    /// Shared durable Agent records.
    pub registry: Box<dyn AgentRuntimeRegistry>,
    /// Shared Workspace records for placement validation.
    pub workspaces: Box<dyn WorkspaceRegistry>,
    /// Shared Project records for active placement validation.
    pub projects: Box<dyn ProjectRegistry>,
    /// Optional metadata coordinator for opening imported session Workspaces.
    pub import_directory: Option<server_metadata::service::directory::Directory>,
    /// Host resource guard, such as the data-directory lease.
    pub lifetime: Arc<dyn Send + Sync>,
}

enum Command {
    Request {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, ErrorCode>>,
        cancel: Option<CancellationToken>,
    },
    Shutdown(oneshot::Sender<Result<(), ErrorCode>>),
}

struct Worker {
    sender: mpsc::Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
    events: SessionEvents,
    timeline: crate::storage::timeline::Timeline,
    creations: server_metadata::service::creation::Creations,
}

// Field drop order keeps the instance lease until the runtime has reaped its children,
// including unwinding paths before the explicit shutdown handshake.
struct OwnedRuntime {
    runtime: tokio::runtime::Runtime,
    _lifetime: Arc<dyn Send + Sync>,
}

/// Cloneable bounded command handle; native sessions outlive individual client connections.
#[derive(Clone)]
pub struct AgentExecution(Arc<Worker>);

impl fmt::Debug for AgentExecution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentExecution")
            .finish_non_exhaustive()
    }
}

impl AgentExecution {
    /// Start a dedicated current-thread runtime that owns all native sessions.
    ///
    /// # Errors
    /// Returns an I/O error if a runtime or worker thread cannot be created.
    pub fn spawn(mut dependencies: ExecutionDependencies) -> Result<Self, std::io::Error> {
        dependencies
            .manager
            .recover_permissions()
            .map_err(std::io::Error::other)?;
        let timeline = match dependencies.manager.timeline() {
            Some(timeline) => timeline,
            None => crate::storage::timeline::Timeline::memory()
                .map_err(|_| std::io::Error::other("initialize timeline"))?,
        };
        timeline
            .recover_inputs()
            .map_err(|_| std::io::Error::other("recover input receipts"))?;
        dependencies.manager = dependencies.manager.with_timeline(timeline.clone());
        let creations = dependencies.manager.creations();
        let events = dependencies.manager.events();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (sender, receiver) = mpsc::channel(64);
        let thread = std::thread::Builder::new()
            .name("server-provider".to_owned())
            .spawn(move || {
                let owned = OwnedRuntime {
                    runtime,
                    _lifetime: dependencies.lifetime,
                };
                let state = ExecutionState {
                    manager: dependencies.manager,
                    directory: dependencies.directory,
                    registry: dependencies.registry,
                    workspaces: dependencies.workspaces,
                    projects: dependencies.projects,
                    import_directory: dependencies.import_directory,
                };
                owned.runtime.block_on(serve(state, receiver));
            })?;
        Ok(Self(Arc::new(Worker {
            sender,
            thread: Mutex::new(Some(thread)),
            events,
            timeline,
            creations,
        })))
    }

    /// Return the installed durable timeline projection and its observers.
    #[must_use]
    pub fn timeline(&self) -> crate::storage::timeline::Timeline {
        self.0.timeline.clone()
    }

    /// Return shared metadata-owned creation receipts.
    #[must_use]
    pub fn creations(&self) -> server_metadata::service::creation::Creations {
        self.0.creations.clone()
    }

    /// Return connection events shared with this worker's Agent manager.
    #[must_use]
    pub fn events(&self) -> SessionEvents {
        self.0.events.clone()
    }

    /// Execute a canonical execution or Agent metadata request.
    ///
    /// Wait requests poll without holding the command lane; cancelling their caller does not
    /// cancel an accepted turn. Input queues and native requests are bounded independently.
    ///
    /// # Errors
    /// Returns safe validation, admission, provider or registry errors.
    pub async fn execute(&self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        if method != "agent.finish.wait.request" {
            return self.call(method, params).await;
        }
        only(&params, &["agentId", "timeoutMs"])?;
        let request: WaitRequest =
            serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
        let timeout = request.timeout_ms.unwrap_or(30_000);
        if timeout == 0 || timeout > 30_000 {
            return Err(ErrorCode::InvalidMessage);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
        loop {
            let mut result = self
                .call(method, json!({"agentId":request.agent_id}))
                .await?;
            if result["status"] != "running" {
                return Ok(result);
            }
            if tokio::time::Instant::now() >= deadline {
                result["status"] = json!("timeout");
                return Ok(result);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Drain accepted commands, close native children, and join the owning thread.
    ///
    /// # Errors
    /// Returns a native close or worker failure. The host lifetime guard remains owned until
    /// the worker has actually terminated, including after the calling future is dropped.
    pub async fn shutdown(&self) -> Result<(), ErrorCode> {
        let (reply, receiver) = oneshot::channel();
        let sent = self.0.sender.send(Command::Shutdown(reply)).await.is_ok();
        let result = if sent {
            receiver.await.unwrap_or(Err(ErrorCode::AgentIo))
        } else {
            Ok(())
        };
        let thread = self.0.thread.lock().map_err(|_| ErrorCode::AgentIo)?.take();
        if let Some(thread) = thread {
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .map_err(|_| ErrorCode::AgentIo)?
                .map_err(|_| ErrorCode::AgentIo)?;
        }
        result
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        self.call_cancellable(method, params, None).await
    }

    async fn call_cancellable(
        &self,
        method: &str,
        params: Value,
        cancel: Option<CancellationToken>,
    ) -> Result<Value, ErrorCode> {
        let (reply, receiver) = oneshot::channel();
        self.0
            .sender
            .try_send(Command::Request {
                method: method.to_owned(),
                params,
                reply,
                cancel,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ErrorCode::CatalogBusy,
                mpsc::error::TrySendError::Closed(_) => ErrorCode::AgentIo,
            })?;
        receiver.await.map_err(|_| ErrorCode::AgentIo)?
    }
}

async fn serve(mut state: ExecutionState, mut commands: mpsc::Receiver<Command>) {
    let mut interval = tokio::time::interval(Duration::from_millis(25));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Request { method, params, reply, cancel }) => {
                    voice::dispatch(&mut state, &method, params, (reply, cancel)).await;
                }
                Some(Command::Shutdown(reply)) => {
                    commands.close();
                    let result = state.manager.close_all().await.map_err(|_| ErrorCode::AgentIo);
                    let _ = reply.send(result);
                    break;
                }
                None => { let _ = state.manager.close_all().await; break; }
            },
            _ = interval.tick() => {
                // Failed durable writes retain their pending event and are retried here.
                let _ = state.manager.poll().await;
                let _ = state.manager.reconcile().await;
                let _ = state.manager.dispatch_pending_inputs().await;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
