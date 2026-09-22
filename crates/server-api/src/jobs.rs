use std::sync::{Arc, Mutex};

use server_protocol::ErrorCode;

use crate::Shared;

pub(super) async fn run<S: Send + 'static, R: Send + 'static>(
    state: &Shared,
    service: Option<Arc<Mutex<S>>>,
    failure: ErrorCode,
    execute: impl FnOnce(&mut S) -> Result<R, ErrorCode> + Send + 'static,
) -> Result<R, ErrorCode> {
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
