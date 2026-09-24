use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::outbound::{Outbound, QueueError};
use crate::{ErrorCode, Runtime, ServerMessage};

/// One validated request admitted by the API.
#[derive(Debug)]
pub struct Request {
    /// Connection-local correlation ID.
    pub id: String,
    /// Canonical method name.
    pub method: String,
    /// Owned business payload.
    pub params: Value,
}

/// Concrete request resources shared by all capability dispatchers.
///
/// Business services and connection subscriptions are passed separately to their owning crate.
#[derive(Debug)]
pub struct Context<'a> {
    /// Admitted request consumed exactly once.
    pub request: Request,
    /// Shared Tokio task, admission and budget resources.
    pub runtime: &'a Runtime,
    /// Connection-owned, bounded response delivery queue.
    pub outbound: &'a Outbound,
    /// Remaining capacity across every subscription kind on this connection.
    pub available_subscriptions: usize,
}

impl Context<'_> {
    /// Execute `operation` with the owned method and parameters using the shared job budget.
    /// # Errors
    /// Returns admission, service-lock, task or converted business errors.
    pub async fn call<S, R, E>(
        &mut self,
        service: Option<Arc<Mutex<S>>>,
        failure: ErrorCode,
        operation: fn(&mut S, &str, Value) -> Result<R, E>,
    ) -> Result<R, ErrorCode>
    where
        S: Send + 'static,
        R: Send + 'static,
        E: Into<ErrorCode> + 'static,
    {
        let method = std::mem::take(&mut self.request.method);
        let params = std::mem::take(&mut self.request.params);
        self.runtime
            .run(service, failure, move |service| {
                operation(service, &method, params).map_err(Into::into)
            })
            .await
    }

    /// Execute one ordinary RPC and send its response or stable error.
    /// # Errors
    /// Returns an encoding or queue failure when the response cannot be delivered.
    pub async fn rpc<S, E>(
        mut self,
        service: Option<Arc<Mutex<S>>>,
        failure: ErrorCode,
        operation: fn(&mut S, &str, Value) -> Result<Value, E>,
    ) -> Result<(), QueueError>
    where
        S: Send + 'static,
        E: Into<ErrorCode> + 'static,
    {
        let result = self.call(service, failure, operation).await;
        self.respond(result)
    }

    /// Send one response or error using this request's correlation ID.
    /// # Errors
    /// Returns an encoding or queue failure; no later action should be activated on failure.
    pub fn respond(self, result: Result<Value, ErrorCode>) -> Result<(), QueueError> {
        self.outbound.respond(self.request.id, result)
    }

    /// Send a successful Workspace response before its optional update event.
    /// # Errors
    /// Returns the first encoding or queue failure.
    pub fn workspace(self, value: Value, event: Option<Value>) -> Result<(), QueueError> {
        let outbound = self.outbound;
        self.respond(Ok(value))?;
        if let Some(params) = event {
            outbound.send(&ServerMessage::Event {
                method: "workspace.update".to_owned(),
                params,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
