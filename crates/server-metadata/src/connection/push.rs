//! Connection-local token ownership and asynchronous persistence admission.

use serde_json::{Value, json};
use server_model::outbound::QueueError;
use server_model::{Context, ErrorCode};

use crate::connection::Connection;
use crate::dispatch::State;
use crate::protocol::push::TokenRequest;
use crate::service::push::PushError;

/// Persist registration before making the token eligible for this connection's heartbeats.
/// # Errors
/// Returns validation, admission or storage errors without echoing token contents.
pub async fn register(
    params: Value,
    state: &State,
    connection: &mut Connection,
) -> Result<(), ErrorCode> {
    let request: TokenRequest =
        serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
    let token = request.token.trim().to_owned();
    renew(state, token.clone()).await?;
    connection.push_token = if token.is_empty() { None } else { Some(token) };
    Ok(())
}

/// Renew the lease associated with this physical connection after a valid heartbeat.
/// # Errors
/// Returns admission or storage errors; a failed write can be retried by a later heartbeat.
pub async fn heartbeat(state: &State, connection: &Connection) -> Result<(), ErrorCode> {
    if let Some(token) = &connection.push_token {
        renew(state, token.clone()).await?;
    }
    Ok(())
}

async fn renew(state: &State, token: String) -> Result<(), ErrorCode> {
    state
        .run(
            state.push_tokens.clone(),
            ErrorCode::RegistryIo,
            move |service| {
                service
                    .renew(&token, chrono::Utc::now().timestamp_millis())
                    .map_err(error)
            },
        )
        .await
}

pub(crate) async fn unregister(
    context: Context<'_>,
    state: &State,
    connection: &mut Connection,
) -> Result<(), QueueError> {
    let result = async {
        let request: TokenRequest = serde_json::from_value(context.request.params.clone())
            .map_err(|_| ErrorCode::InvalidMessage)?;
        let token = request.token.trim().to_owned();
        let revoke = token.clone();
        state
            .run(
                state.push_tokens.clone(),
                ErrorCode::RegistryIo,
                move |service| service.revoke(&revoke).map_err(error),
            )
            .await?;
        if connection.push_token.as_deref() == Some(token.as_str()) {
            connection.push_token = None;
        }
        Ok(json!({}))
    }
    .await;
    context.respond(result)
}

fn error(error: PushError) -> ErrorCode {
    match error {
        PushError::Invalid => ErrorCode::InvalidMessage,
        PushError::Io => ErrorCode::RegistryIo,
        PushError::Capacity => ErrorCode::ResourceExhausted,
    }
}
