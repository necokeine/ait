//! Browser request dispatch; callbacks are owned by the physical connection.
use crate::{broker::Broker, capabilities::Group, connection::Connection};
use server_model::{Context, ErrorCode, outbound::QueueError};
/// Host-composed automation broker.
#[derive(Debug)]
pub struct State {
    /// Shared broker, absent in capability-limited hosts.
    pub broker: Option<Broker>,
}
/// Register a host using one connection subscription slot.
/// # Errors
/// Returns outbound queue failures after returning business errors in the response.
pub fn dispatch(
    group: Group,
    mut context: Context<'_>,
    state: &State,
    connection: &mut Connection,
) -> Result<(), QueueError> {
    let Group::Browser = group;
    let result = if context.runtime.cancellation.is_cancelled() {
        Err(ErrorCode::ServerDraining)
    } else if context.available_subscriptions == 0 {
        Err(ErrorCode::ResourceExhausted)
    } else if let Some(broker) = &state.broker {
        connection.register(
            broker,
            std::mem::take(&mut context.request.params),
            context.outbound.clone(),
        )
    } else {
        Err(ErrorCode::UnsupportedCapability)
    };
    context.respond(result)
}
