use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};

use super::*;

type RequestBodies = Vec<(HeaderMap, Vec<u8>)>;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<RequestBodies>>);

async fn transcription(State(state): State<Captured>, headers: HeaderMap, body: Bytes) -> String {
    state.0.lock().unwrap().push((headers, body.to_vec()));
    r#"{"text":"hello","language":"en"}"#.to_owned()
}

async fn speech(State(state): State<Captured>, headers: HeaderMap, body: Bytes) -> Vec<u8> {
    state.0.lock().unwrap().push((headers, body.to_vec()));
    vec![0, 0, 1, 0]
}

fn config(url: String) -> Config {
    Config {
        base_url: url,
        api_key: Some("offline-secret".into()),
        stt_model: "test-stt".to_owned(),
        tts_model: "test-tts".to_owned(),
        voice: "alloy".to_owned(),
    }
}

#[tokio::test]
async fn http_adapter_uses_real_multipart_and_pcm_speech_contracts() {
    let captured = Captured::default();
    let app = Router::new()
        .route("/v1/audio/transcriptions", post(transcription))
        .route("/v1/audio/speech", post(speech))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let engine = OpenAi::new(config(format!(
        "http://{}/v1/",
        listener.local_addr().unwrap()
    )))
    .unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let transcript = engine
        .transcribe(
            Audio {
                bytes: vec![0; 32],
                format: Format::Pcm(16000),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(transcript.text, "hello");
    let audio = engine
        .synthesize("hello", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(audio.format, Format::Pcm(24000));
    let calls = captured.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0["authorization"], "Bearer offline-secret");
    let multipart = String::from_utf8_lossy(&calls[0].1);
    assert!(multipart.contains("name=\"model\"\r\n\r\ntest-stt"));
    assert!(multipart.contains("speech.wav"));
    assert!(calls[0].1.windows(4).any(|window| window == b"RIFF"));
    let payload: serde_json::Value = serde_json::from_slice(&calls[1].1).unwrap();
    assert_eq!(payload["response_format"], "pcm");
    assert_eq!(payload["input"], "hello");
    assert!(!format!("{engine:?}").contains("offline-secret"));
    task.abort();
}

#[tokio::test]
async fn http_errors_and_cancellation_do_not_expose_provider_bodies() {
    let app = Router::new().route(
        "/v1/audio/speech",
        post(|| async { (StatusCode::UNAUTHORIZED, "private credential diagnostic") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let engine = OpenAi::new(config(format!(
        "http://{}/v1",
        listener.local_addr().unwrap()
    )))
    .unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert_eq!(
        engine
            .synthesize("hello", CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Unavailable
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        engine.synthesize("hello", cancel).await.unwrap_err(),
        Error::Cancelled
    );
    assert_eq!(
        engine
            .synthesize("", CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Invalid
    );
    task.abort();
}

#[test]
fn endpoint_configuration_rejects_credentials_queries_and_invalid_schemes() {
    for url in [
        "ftp://example.com",
        "https://user:secret@example.com",
        "https://example.com?token=secret",
        "https://example.com/#fragment",
        "invalid",
    ] {
        assert!(OpenAi::new(config(url.to_owned())).is_err());
    }
}

#[tokio::test]
async fn provider_response_limits_and_bad_audio_are_rejected() {
    let app = Router::new()
        .route("/too-large", post(|| async { vec![0_u8; 32] }))
        .route(
            "/failure",
            post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .route("/v1/audio/speech", post(|| async { vec![0_u8; 3] }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = Client::new();
    assert_eq!(
        response(
            client.post(format!("http://{address}/too-large")),
            CancellationToken::new(),
            8
        )
        .await
        .unwrap_err(),
        Error::Capacity
    );
    assert_eq!(
        response(
            client.post(format!("http://{address}/failure")),
            CancellationToken::new(),
            8
        )
        .await
        .unwrap_err(),
        Error::Provider
    );
    let engine = OpenAi::new(config(format!("http://{address}/v1"))).unwrap();
    assert_eq!(
        engine
            .synthesize("hello", CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Provider
    );
    task.abort();
}

#[tokio::test]
async fn long_multibyte_synthesis_is_split_without_losing_text() {
    let captured = Captured::default();
    let app = Router::new()
        .route("/v1/audio/speech", post(speech))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let engine = OpenAi::new(config(format!(
        "http://{}/v1",
        listener.local_addr().unwrap()
    )))
    .unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let text = "听写".repeat(2001);
    assert_eq!(
        engine
            .synthesize(&text, CancellationToken::new())
            .await
            .unwrap()
            .bytes
            .len(),
        12
    );
    let calls = captured.0.lock().unwrap();
    let inputs = calls
        .iter()
        .map(|(_, body)| {
            serde_json::from_slice::<serde_json::Value>(body).unwrap()["input"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert!(inputs.iter().all(|input| input.chars().count() <= 2000));
    assert_eq!(inputs.concat(), text);
    task.abort();
}
