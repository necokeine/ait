//! Request validation and Paseo result projection independent of transport envelopes.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::Error;
use crate::protocol::{CaptureRequest, CreateRequest, ListRequest, RenameRequest, TerminalRequest};
use crate::service::Terminals;

/// Decode one method payload, preserving serde's integer and enum validation.
///
/// # Errors
/// Returns `Error::Invalid` for an invalid payload.
pub fn decode<T: DeserializeOwned>(params: Value) -> Result<T, Error> {
    serde_json::from_value(params).map_err(|_| Error::Invalid)
}

/// Execute the five stateless RPCs; subscription/input operations belong to the connection.
///
/// # Errors
/// Returns malformed payloads, unknown methods, registry failures, or process errors.
pub fn execute(service: &mut Terminals, method: &str, params: Value) -> Result<Value, Error> {
    match method {
        "terminal.list.request" => {
            let request: ListRequest = decode(params)?;
            let terminals = service.list(&request)?;
            let mut result = json!({"terminals":terminals});
            if let Some(cwd) = request.cwd {
                result["cwd"] = json!(cwd);
            }
            Ok(result)
        }
        "terminal.create.request" => {
            let request: CreateRequest = decode(params)?;
            Ok(match service.create(&request) {
                Ok(terminal) => json!({"terminal":terminal,"error":null}),
                Err(error) => json!({"terminal":null,"error":error.to_string()}),
            })
        }
        "terminal.rename.request" => {
            let request: RenameRequest = decode(params)?;
            Ok(match service.rename(&request.terminal_id, &request.title) {
                Ok(()) => json!({"success":true,"error":null}),
                Err(error) => json!({"success":false,"error":error.to_string()}),
            })
        }
        "terminal.kill.request" => {
            let request: TerminalRequest = decode(params)?;
            let success = service.kill(&request.terminal_id).is_ok();
            Ok(json!({"terminalId":request.terminal_id,"success":success}))
        }
        "terminal.capture.request" => {
            let request: CaptureRequest = decode(params)?;
            let lines = service.capture(&request.terminal_id)?;
            let total = lines.len();
            let start = index(request.start, total, 0);
            let end = index(request.end, total, total.saturating_sub(1));
            let selected = if total == 0 || start > end {
                &[]
            } else {
                &lines[start..=end]
            };
            Ok(json!({"terminalId":request.terminal_id,"lines":selected,"totalLines":total}))
        }
        _ => Err(Error::MethodNotFound),
    }
}

fn index(index: Option<i64>, total: usize, default: usize) -> usize {
    let Some(index) = index else {
        return default;
    };
    let total = i64::try_from(total).unwrap_or(i64::MAX);
    let index = if index < 0 {
        total.saturating_add(index)
    } else {
        index
    };
    usize::try_from(index.max(0).min(total.saturating_sub(1).max(0))).unwrap_or(0)
}

#[cfg(test)]
mod tests;
