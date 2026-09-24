use serde::Serialize;
use serde_json::Value;
use server_filesystem::rpc::files::project_version;
use server_filesystem::service::files as port;
use server_protocol::ErrorCode;

use crate::Shared;
mod connection;
mod transfer;
pub(super) use connection::{FileConnection, FileRequest};
pub(super) use transfer::download;
async fn execute(method: String, params: Value, state: &Shared) -> Result<Value, ErrorCode> {
    crate::jobs::run(
        state,
        state.files.clone(),
        ErrorCode::ProjectIo,
        move |files| {
            server_filesystem::rpc::files::dispatch(files, &method, params).map_err(Into::into)
        },
    )
    .await
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::ProjectIo)
}
