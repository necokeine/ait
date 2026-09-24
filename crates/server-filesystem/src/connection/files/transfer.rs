use crate::protocol::{
    file_transfer::{self, FileFrame},
    files as wire,
};
use crate::service::files::FileError;
use crate::service::transfer::Cursor;
use serde_json::Value;
use server_model::ErrorCode;
use tokio_util::task::TaskTracker;

use crate::dispatch::State as Shared;
use server_model::outbound::{Outbound, QueueError};

pub(crate) async fn stream_preview(
    id: String,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
) -> Result<(), QueueError> {
    let request: wire::ExplorerRequest = match super::decode(params) {
        Ok(request) => request,
        Err(error) => return super::connection::respond(outbound, id, Err(error)),
    };
    if request.max_bytes == Some(0) {
        return super::connection::respond(outbound, id, Err(ErrorCode::InvalidMessage));
    }
    let cwd = request.cwd.trim().to_owned();
    let path = request.path.unwrap_or_else(|| ".".to_owned());
    let open_cwd = cwd.clone();
    let open_path = path.clone();
    let preview = state
        .run(state.files.clone(), ErrorCode::ProjectIo, move |files| {
            Ok(crate::rpc::files::binary_preview(
                files,
                &open_cwd,
                &open_path,
                request.max_bytes,
            ))
        })
        .await;
    let (metadata, mut cursor) = match preview {
        Ok(Ok(preview)) => preview,
        Ok(Err(error)) => return explorer_error(outbound, id, cwd, path, error.0),
        Err(error) => return super::connection::respond(outbound, id, Err(error)),
    };
    send_frame(outbound, &id, &FileFrame::Begin(metadata)).await?;
    loop {
        let result = chunk(cursor, &state.tasks).await;
        match result {
            Ok((next, Some(bytes))) => {
                cursor = next;
                send_frame(outbound, &id, &FileFrame::Chunk(bytes)).await?;
            }
            Ok((_, None)) => return send_frame(outbound, &id, &FileFrame::End).await,
            Err(error) => return explorer_error(outbound, id, cwd, path, error.0),
        }
        if state.cancellation.is_cancelled() {
            return Ok(());
        }
    }
}

async fn send_frame(outbound: &Outbound, id: &str, frame: &FileFrame) -> Result<(), QueueError> {
    let bytes = file_transfer::encode(id, frame).map_err(|_| QueueError::Full)?;
    outbound.binary(bytes).await
}

fn explorer_error(
    outbound: &Outbound,
    id: String,
    cwd: String,
    path: String,
    error: String,
) -> Result<(), QueueError> {
    super::connection::respond(
        outbound,
        id,
        super::encode(wire::ExplorerResult {
            cwd,
            path,
            mode: wire::ExplorerMode::File,
            directory: None,
            file: None,
            error: Some(error),
        }),
    )
}

/// Read one bounded chunk while tracking blocking work through server drain.
/// # Errors
/// Returns cursor I/O or blocking-task failures.
pub async fn chunk(
    cursor: Cursor,
    tasks: &TaskTracker,
) -> Result<(Cursor, Option<Vec<u8>>), FileError> {
    let tracking = tasks.token();
    tokio::task::spawn_blocking(move || {
        let _tracking = tracking;
        cursor.read_chunk()
    })
    .await
    .map_err(|_| FileError("File transfer task failed".to_owned()))?
}
