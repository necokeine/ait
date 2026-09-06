//! Rig 0.42 requires string content and an index on each `DeepSeek` tool call.
//! The API permits null assistant content and omits indices in non-streaming
//! calls. Normalize just those fields before Rig deserializes the response.

use bytes::Bytes;
use rig::http_client::{
    HttpClientExt, LazyBody, MultipartForm, Request, Response, Result, StreamingResponse,
};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub(super) struct DeepSeekHttp(pub reqwest::Client);

impl HttpClientExt for DeepSeekHttp {
    fn send<T, U>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + Send + 'static
    where
        T: Into<Bytes> + Send,
        U: From<Bytes> + Send + 'static,
    {
        let completion =
            request.method() == "POST" && request.uri().path().ends_with("/chat/completions");
        let response = self.0.send::<T, Bytes>(request);
        async move {
            let response = response.await?;
            let (mut parts, body) = response.into_parts();
            // The transformed lazy body can have a different byte length.
            if completion {
                parts.headers.remove("content-length");
            }
            let body: LazyBody<U> = Box::pin(async move {
                let bytes = body.await?;
                Ok(U::from(if completion { normalize(bytes) } else { bytes }))
            });
            Ok(Response::from_parts(parts, body))
        }
    }

    fn send_multipart<U>(
        &self,
        request: Request<MultipartForm>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + Send + 'static
    where
        U: From<Bytes> + Send + 'static,
    {
        self.0.send_multipart(request)
    }

    fn send_streaming<T>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<StreamingResponse>> + Send
    where
        T: Into<Bytes> + Send,
    {
        self.0.send_streaming(request)
    }
}

fn normalize(bytes: Bytes) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    let Some(choices) = value.get_mut("choices").and_then(Value::as_array_mut) else {
        return bytes;
    };
    for choice in choices {
        let Some(message) = choice.get_mut("message").and_then(Value::as_object_mut) else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if message.get("content").is_none_or(Value::is_null) {
            message.insert("content".into(), Value::String(String::new()));
        }
        if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for (index, call) in calls.iter_mut().enumerate() {
                if let Some(call) = call.as_object_mut() {
                    call.entry("index").or_insert_with(|| Value::from(index));
                }
            }
        }
    }
    serde_json::to_vec(&value).map_or(bytes, Bytes::from)
}
