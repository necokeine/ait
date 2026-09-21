use std::sync::{Arc, Mutex};

use serde_json::Value;
use server_protocol::ErrorCode;

use crate::Shared;

pub(super) async fn run<T: Send + 'static>(
    state: &Shared,
    service: Option<Arc<Mutex<T>>>,
    failure: ErrorCode,
    execute: impl FnOnce(&mut T) -> Result<Value, ErrorCode> + Send + 'static,
) -> Result<Value, ErrorCode> {
    let service = service.ok_or(ErrorCode::UnsupportedCapability)?;
    // One short catalog/project job at a time. Tracking and admission survive
    // a disconnected response future and are serialized with shutdown.
    let admission = state
        .admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.cancellation.is_cancelled() {
        return Err(ErrorCode::ServerDraining);
    }
    let permit = state
        .jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| ErrorCode::ResourceExhausted)?;
    let tracking = state.tasks.token();
    let job = tokio::task::spawn_blocking(move || {
        let (_tracking, _permit) = (tracking, permit);
        let mut service = service.lock().map_err(|_| failure)?;
        execute(&mut service)
    });
    drop(admission);
    job.await.map_err(|_| failure)?
}
