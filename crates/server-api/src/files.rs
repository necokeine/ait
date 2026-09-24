use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use server_filesystem::service::transfer::Cursor;
use server_protocol::ErrorCode;

use crate::Shared;

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
    let result = state
        .run(
            state.filesystem.files.clone(),
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
    let cursor = Cursor::new(reader);
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
            match server_filesystem::connection::files::transfer::chunk(cursor, &tasks).await {
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
