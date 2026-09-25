use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{Json, Router, extract::State, routing::post};
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::{
    Command, Process, Stdio, TOKEN, ready, terminate,
    transport::{connect, receive, request},
};

async fn transcription(State(hang): State<Arc<AtomicBool>>) -> Json<Value> {
    Json(json!({"text":if hang.load(Ordering::SeqCst) { "hang" } else { "hello voice" }}))
}

async fn speech(Json(payload): Json<Value>) -> Vec<u8> {
    assert_eq!(payload["input"], "Echo: hello voice");
    assert_eq!(payload["response_format"], "pcm");
    vec![0, 0, 1, 0]
}

fn start_server(
    root: &std::path::Path,
    path: &std::ffi::OsStr,
    endpoint: &str,
) -> (Process, std::path::PathBuf) {
    let log = root.join("server.log");
    let child = Command::new(env!("CARGO_BIN_EXE_server"))
        .arg("--data-dir")
        .arg(root.join("state"))
        .args(["--listen", "127.0.0.1:0"])
        .env("AIT_SERVER_TOKEN", TOKEN)
        .env("PATH", path)
        .env("AIT_SPEECH_PROVIDER", "openai")
        .env("AIT_SPEECH_BASE_URL", endpoint)
        .env("AIT_SPEECH_API_KEY", "offline-voice-secret")
        .env_remove("AIT_SPEECH_STT_PROVIDER")
        .env_remove("AIT_SPEECH_TTS_PROVIDER")
        .env_remove("AIT_SERVER_LISTEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    (Process(child), log)
}

#[tokio::test]
async fn production_voice_calls_speech_and_native_agent_then_disconnect_interrupts_owned_turn() {
    let super::native::NativeFixture { root, cwd, path } = super::native::NativeFixture::new();
    let hang = Arc::new(AtomicBool::new(false));
    let app = Router::new()
        .route("/v1/audio/transcriptions", post(transcription))
        .route("/v1/audio/speech", post(speech))
        .with_state(hang.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let http = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (mut process, log) = start_server(root.path(), &path, &endpoint);
    let address = ready(&mut process, &log).await;
    let mut control = connect(
        &address,
        &[
            "workspace.open.request",
            "agent.create.request",
            "agent.get.request",
            "agent.finish.wait.request",
        ],
    )
    .await;
    assert_eq!(
        request(&mut control, "workspace.open.request", json!({"cwd":cwd})).await["type"],
        "response"
    );
    let agent = request(
        &mut control,
        "agent.create.request",
        json!({"config":{"provider":"codex","cwd":cwd}}),
    )
    .await;
    let id = agent["result"]["agentId"].as_str().unwrap().to_owned();
    let mut client = connect(&address, server_voice::protocol::CAPABILITIES).await;
    let mode = request(
        &mut client,
        "voice.mode.set.request",
        json!({"enabled":true,"agentId":id}),
    )
    .await;
    assert_eq!(mode["result"]["accepted"], true, "{mode}");
    let audio = json!({"type":"event","method":"voice.audio.chunk","params":{"audio":"AAgACA==","format":"audio/pcm;rate=16000","isLast":true}});
    client
        .send(Message::Text(audio.to_string().into()))
        .await
        .unwrap();
    let mut transcript_seen = false;
    loop {
        let event = receive(&mut client).await;
        match event["method"].as_str() {
            Some("voice.transcription.result") => {
                assert_eq!(event["params"]["text"], "hello voice");
                transcript_seen = true;
            }
            Some("voice.audio.output") => {
                assert!(transcript_seen);
                assert_eq!(event["params"]["audio"], "AAABAA==");
                client.send(Message::Text(json!({"type":"event","method":"voice.audio.played","params":{"id":event["params"]["id"]}}).to_string().into())).await.unwrap();
                break;
            }
            Some("voice.input.state") => {}
            _ => panic!("unexpected voice event: {event}"),
        }
    }
    hang.store(true, Ordering::SeqCst);
    client
        .send(Message::Text(audio.to_string().into()))
        .await
        .unwrap();
    loop {
        if receive(&mut client).await["method"] == "voice.transcription.result" {
            break;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if request(&mut control, "agent.get.request", json!({"agentId":id})).await["result"]["agent"]["status"] == "running" { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    client.close(None).await.unwrap();
    let final_state = request(
        &mut control,
        "agent.finish.wait.request",
        json!({"agentId":id,"timeoutMs":5000}),
    )
    .await;
    assert_eq!(final_state["result"]["status"], "idle", "{final_state}");
    terminate(&mut process).await;
    assert!(
        !std::fs::read_to_string(&log)
            .unwrap()
            .contains("offline-voice-secret")
    );
    http.abort();
}
