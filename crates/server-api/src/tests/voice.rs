use server_voice::{
    Error,
    audio::Audio,
    ports::{Operation, Transcriber, Transcript},
    service::Speech,
};
use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Debug)]
struct Engine;

impl Transcriber for Engine {
    fn transcribe(&self, audio: Audio, _: CancellationToken) -> Operation<'_, Transcript> {
        Box::pin(async move {
            if audio.bytes != [0, 0, 1, 0] {
                return Err(Error::Invalid);
            }
            Ok(Transcript {
                text: "socket transcript".to_owned(),
                language: None,
            })
        })
    }
}

async fn voice_socket(fixture: &Fixture) -> Socket {
    let mut socket = fixture.socket().await;
    let mut offer = hello();
    offer["capabilities"] = json!(server_voice::protocol::CAPABILITIES);
    send(&mut socket, offer).await;
    let info = receive(&mut socket).await;
    assert_eq!(info["negotiated_capabilities"].as_array().unwrap().len(), 8);
    socket
}

#[tokio::test]
async fn dictation_roundtrip_is_real_and_scoped_to_the_physical_socket() {
    let services = Services {
        speech: Some(Speech::new(Some(Arc::new(Engine)), None, None)),
        ..Services::default()
    };
    let fixture = Fixture::with_services(services).await;
    let mut first = voice_socket(&fixture).await;
    let mut second = voice_socket(&fixture).await;
    send(&mut first, json!({"type":"event","method":"dictation.stream.start","params":{"dictationId":"d1","format":"pcm"}})).await;
    assert_eq!(receive(&mut first).await["params"]["ackSeq"], -1);
    send(&mut second, json!({"type":"event","method":"dictation.stream.finish","params":{"dictationId":"d1","finalSeq":0}})).await;
    assert_eq!(
        receive(&mut second).await["method"],
        "dictation.stream.error"
    );
    send(&mut first, json!({"type":"event","method":"dictation.stream.chunk","params":{"dictationId":"d1","seq":0,"audio":"AAABAA==","format":"pcm"}})).await;
    assert_eq!(receive(&mut first).await["params"]["ackSeq"], 0);
    send(&mut first, json!({"type":"event","method":"dictation.stream.finish","params":{"dictationId":"d1","finalSeq":0}})).await;
    assert_eq!(
        receive(&mut first).await["method"],
        "dictation.stream.finish.accepted"
    );
    let final_result = receive(&mut first).await;
    assert_eq!(final_result["method"], "dictation.stream.final");
    assert_eq!(final_result["params"]["text"], "socket transcript");
    send(
        &mut first,
        json!({"type":"event","method":"dictation.stream.cancel","params":{"dictationId":"d1"}}),
    )
    .await;
    let mode = request(
        &mut first,
        "voice.mode.set.request",
        json!({"enabled":true,"agentId":"agent"}),
    )
    .await;
    assert_eq!(mode["result"]["reasonCode"], "speech_backend_unavailable");
    assert_eq!(
        request(&mut first, "voice.abort.request", json!({})).await["result"]["accepted"],
        true
    );
    assert_eq!(receive(&mut first).await["method"], "voice.input.state");
    drop(first);
    drop(second);
    fixture.stop().await;
}

#[tokio::test]
async fn speech_envelope_and_negotiation_errors_do_not_reach_engines() {
    let fixture = Fixture::with_services(Services {
        speech: Some(Speech::new(None, None, None)),
        ..Services::default()
    })
    .await;
    let mut socket = fixture.socket().await;
    send(&mut socket, hello()).await;
    receive(&mut socket).await;
    send(&mut socket, json!({"type":"event","method":"dictation.stream.start","params":{"dictationId":"d1","format":"pcm"}})).await;
    assert_eq!(receive(&mut socket).await["code"], "unsupported_capability");
    assert_eq!(
        request(&mut socket, "voice.audio.chunk", json!({})).await["code"],
        "invalid_message"
    );
    assert_eq!(
        request(&mut socket, "set_voice_mode", json!({})).await["code"],
        "method_not_found"
    );
    send(
        &mut socket,
        json!({"type":"event","method":"voice.abort.request","params":{}}),
    )
    .await;
    assert_eq!(receive(&mut socket).await["code"], "invalid_message");
    drop(socket);
    fixture.stop().await;
}
