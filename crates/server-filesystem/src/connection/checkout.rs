use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::rpc::checkout::DiffObservation;
use crate::service::checkout::{self as port, Checkout};
use serde_json::Value;
use server_model::{ErrorCode, ServerMessage};
use tokio::sync::Semaphore;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::dispatch::State as Shared;
use server_model::outbound::Outbound;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

pub struct Dispatch {
    pub value: Value,
    pub subscription: Option<PendingSubscription>,
}

pub struct PendingSubscription {
    observation: DiffObservation,
    service: Arc<Mutex<Checkout>>,
    jobs: Arc<Semaphore>,
    tracker: TaskTracker,
    server_cancel: CancellationToken,
    outbound: Outbound,
}

/// RAII owner for one connection-local diff polling task.
pub struct CheckoutDiffSubscription {
    cancellation: CancellationToken,
}

impl Drop for CheckoutDiffSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl PendingSubscription {
    pub fn activate(self) -> (String, CheckoutDiffSubscription) {
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let subscription_id = self.observation.id().to_owned();
        self.tracker.spawn(async move {
            let mut observation = self.observation;
            let mut interval =
                tokio::time::interval_at(Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = task_cancellation.cancelled() => break,
                    () = self.server_cancel.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let Some(snapshot) = poll_diff(
                    self.service.clone(),
                    self.jobs.clone(),
                    observation.cwd().to_owned(),
                    observation.compare().clone(),
                )
                .await
                else {
                    continue;
                };
                let params = match observation.update(snapshot) {
                    Ok(Some(params)) => params,
                    Ok(None) => continue,
                    Err(_) => break,
                };
                if self
                    .outbound
                    .send(&ServerMessage::Event {
                        method: "checkout.diff.update".to_owned(),
                        params,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        (subscription_id, CheckoutDiffSubscription { cancellation })
    }
}

pub async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: Outbound,
) -> Result<Dispatch, ErrorCode> {
    if method == "checkout.diff.subscribe.request" {
        return subscribe(params, state, outbound).await;
    }
    let method = method.to_owned();
    state
        .run(
            state.checkout.clone(),
            ErrorCode::ProjectIo,
            move |checkout| {
                let value = crate::rpc::checkout::execute(checkout, &method, params)?;
                Ok(Dispatch {
                    value,
                    subscription: None,
                })
            },
        )
        .await
}

async fn subscribe(
    params: Value,
    state: &Shared,
    outbound: Outbound,
) -> Result<Dispatch, ErrorCode> {
    let (observation, value) = state
        .run(
            state.checkout.clone(),
            ErrorCode::ProjectIo,
            move |checkout| DiffObservation::prepare(checkout, params).map_err(Into::into),
        )
        .await?;
    Ok(Dispatch {
        value,
        subscription: Some(PendingSubscription {
            observation,
            service: state
                .checkout
                .clone()
                .ok_or(ErrorCode::UnsupportedCapability)?,
            jobs: state.jobs.clone(),
            tracker: state.tasks.clone(),
            server_cancel: state.cancellation.clone(),
            outbound,
        }),
    })
}

async fn poll_diff(
    service: Arc<Mutex<Checkout>>,
    jobs: Arc<Semaphore>,
    cwd: String,
    compare: port::CheckoutDiffCompare,
) -> Option<Result<port::CheckoutDiff, port::CheckoutRuntimeError>> {
    let permit = jobs.try_acquire_owned().ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let checkout = service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        checkout.diff(&cwd, &compare)
    })
    .await
    .ok()
}
