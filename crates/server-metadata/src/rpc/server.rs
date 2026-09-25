//! Stateless connection metadata requests.

use serde_json::Value;

use super::ErrorCode;
use crate::protocol::server::Ping;

/// Validate and echo an application ping nonce.
///
/// # Errors
/// Rejects malformed, empty, oversized or control-character-containing nonces.
pub fn ping(params: Value) -> Result<Value, ErrorCode> {
    if !params.is_object() {
        return Err(ErrorCode::InvalidMessage);
    }
    let ping: Ping = serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
    if ping.nonce.is_empty() || ping.nonce.len() > 128 || ping.nonce.chars().any(char::is_control) {
        return Err(ErrorCode::InvalidMessage);
    }
    serde_json::to_value(ping).map_err(|_| ErrorCode::InvalidMessage)
}

#[cfg(test)]
mod tests;
