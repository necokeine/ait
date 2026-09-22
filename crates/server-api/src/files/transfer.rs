use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use serde_json::Value;
use server_application::files::{FileError, FileKind, FileReader};
use server_protocol::{
    ErrorCode,
    file_transfer::{self, FileBegin, FileFrame},
    files as wire,
};
use tokio_util::task::TaskTracker;

use crate::{
    Shared,
    outbound::{Outbound, QueueError},
};

pub(super) async fn stream_preview(
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
    let reader = crate::jobs::run(
        state,
        state.files.clone(),
        ErrorCode::ProjectIo,
        move |files| Ok(files.filesystem.open(&open_cwd, &open_path)),
    )
    .await;
    let reader = match reader {
        Ok(Ok(reader)) => reader,
        Ok(Err(error)) => return explorer_error(outbound, id, cwd, path, error.0),
        Err(error) => return super::connection::respond(outbound, id, Err(error)),
    };
    let info = reader.info().clone();
    if request.max_bytes.is_some_and(|limit| info.size > limit) {
        return explorer_error(
            outbound,
            id,
            cwd,
            path,
            "File is too large to display".to_owned(),
        );
    }
    let metadata = FileBegin {
        mime: info.mime_type,
        size: info.size,
        encoding: if info.kind == FileKind::Text {
            "utf-8"
        } else {
            "binary"
        }
        .to_owned(),
        modified_at: info.modified_at,
        revision: Some(info.revision),
        file_name: None,
    };
    send_frame(outbound, &id, &FileFrame::Begin(metadata)).await?;
    let mut cursor = Cursor {
        reader,
        remaining: info.size,
    };
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

struct Cursor {
    reader: Box<dyn FileReader>,
    remaining: u64,
}

async fn chunk(
    mut cursor: Cursor,
    tasks: &TaskTracker,
) -> Result<(Cursor, Option<Vec<u8>>), FileError> {
    let tracking = tasks.token();
    tokio::task::spawn_blocking(move || {
        let _tracking = tracking;
        if cursor.remaining == 0 {
            cursor.reader.verify()?;
            return Ok((cursor, None));
        }
        let mut bytes = vec![
            0;
            usize::try_from(cursor.remaining)
                .unwrap_or(usize::MAX)
                .min(file_transfer::CHUNK_BYTES)
        ];
        cursor.reader.read_exact(&mut bytes)?;
        cursor.remaining -= bytes.len() as u64;
        Ok((cursor, Some(bytes)))
    })
    .await
    .map_err(|_| FileError("File transfer task failed".to_owned()))?
}

pub(crate) async fn download(
    State(state): State<Arc<Shared>>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let Some(token) = query.get("token").filter(|token| !token.trim().is_empty()) else {
        return (StatusCode::BAD_REQUEST, "Missing download token").into_response();
    };
    if query.len() != 1 {
        return (StatusCode::BAD_REQUEST, "Unexpected download parameter").into_response();
    }
    let token = token.trim().to_owned();
    let result = crate::jobs::run(
        &state,
        state.files.clone(),
        ErrorCode::ProjectIo,
        move |files| Ok(files.consume_download(&token)),
    )
    .await;
    let (reader, file_name) = match result {
        Ok(Ok(download)) => download,
        Ok(Err(error)) if error.0 == "Invalid or expired token" => {
            return (StatusCode::FORBIDDEN, error.0).into_response();
        }
        Ok(Err(_)) => return (StatusCode::NOT_FOUND, "File not found").into_response(),
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "File service unavailable").into_response();
        }
    };
    let info = reader.info().clone();
    let cursor = Cursor {
        reader,
        remaining: info.size,
    };
    let tasks = state.tasks.clone();
    let cancel = state.cancellation.clone();
    let body = stream::unfold(Some(cursor), move |cursor| {
        let tasks = tasks.clone();
        let cancel = cancel.clone();
        async move {
            let cursor = cursor?;
            if cancel.is_cancelled() {
                return None;
            }
            match chunk(cursor, &tasks).await {
                Ok((next, Some(bytes))) => Some((Ok::<_, std::io::Error>(bytes), Some(next))),
                Ok((_, None)) => None,
                Err(error) => Some((Err(std::io::Error::other(error.0)), None)),
            }
        }
    });
    let name: String = file_name
        .chars()
        .map(|character| {
            if character.is_ascii()
                && !matches!(character, '"' | '\r' | '\n' | '\\')
                && !character.is_control()
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let mut response = Body::from_stream(body).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = info.mime_type.parse() {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = info.size.to_string().parse() {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    if let Ok(value) = format!("attachment; filename=\"{name}\"").parse() {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        "x-content-type-options",
        axum::http::HeaderValue::from_static("nosniff"),
    );
    response
}
