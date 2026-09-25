//! Schedule request dispatch with bounded, connection-independent execution.
use crate::{capabilities::Group, service::Schedules};
use server_model::{Context, ErrorCode, outbound::QueueError};

/// Host-composed schedule service.
#[derive(Debug)]
pub struct State {
    /// Persistent scheduler, absent in capability-limited hosts.
    pub schedules: Option<Schedules>,
}
/// Execute schedule requests; run-once waits do not block subsequent connection messages.
/// # Errors
/// Returns outbound queue failures. Business failures use the stable schedule RPC error code.
pub async fn dispatch(
    group: Group,
    mut context: Context<'_>,
    state: &State,
) -> Result<(), QueueError> {
    let Group::Schedule = group;
    let Some(schedules) = &state.schedules else {
        return context.respond(Err(ErrorCode::UnsupportedCapability));
    };
    let admission = context
        .runtime
        .admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if context.runtime.cancellation.is_cancelled() {
        return context.respond(Err(ErrorCode::ServerDraining));
    }
    if context.request.method == "schedule.run_once.request" {
        let Ok(permit) = context.runtime.execution_waits.clone().try_acquire_owned() else {
            return context.respond(Err(ErrorCode::ResourceExhausted));
        };
        let schedules = schedules.clone();
        let outbound = context.outbound.clone();
        let failure = outbound.failure();
        let cancel = context.runtime.cancellation.clone();
        let request = context.request;
        context.runtime.tasks.spawn(async move {
            let _permit = permit;
            tokio::select! {
                () = cancel.cancelled() => {},
                () = failure.cancelled() => {},
                result = schedules.execute(&request.method, request.params) => {
                    let _ = outbound.respond(request.id, result.map_err(|_| ErrorCode::ScheduleRequestFailed));
                }
            }
        });
        drop(admission);
        return Ok(());
    }
    drop(admission);
    let params = std::mem::take(&mut context.request.params);
    let method = context.request.method.clone();
    let result = schedules.execute(&method, params).await;
    context.respond(result.map_err(|_| ErrorCode::ScheduleRequestFailed))
}
