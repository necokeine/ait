use super::*;

#[tokio::test]
async fn voice_roundtrip_outputs_transcript_and_acknowledgment_gated_audio() {
    let mut fixture = Fixture::new();
    assert_eq!(fixture.mode(true).await["agentId"], "agent-1");
    fixture.event("voice.audio.chunk", json!({"audio":STANDARD.encode([0, 8, 0, 8]),"format":"audio/pcm;rate=16000","isLast":true}));
    assert_eq!(
        fixture.until("voice.transcription.result").await["text"],
        "recognized speech"
    );
    let first = fixture.until("voice.audio.output").await;
    assert_eq!(first["chunkIndex"], 0);
    assert_eq!(first["isVoiceMode"], true);
    for _ in 0..8 {
        fixture.poll();
    }
    assert_eq!(fixture.connection.voice.acknowledgments.len(), 4);
    fixture.event("voice.audio.played", json!({"id":first["id"]}));
    fixture.poll();
    assert_eq!(fixture.connection.voice.acknowledgments.len(), 4);
    assert_eq!(fixture.mode(false).await["enabled"], false);
    assert!(fixture.connection.is_empty());
}

#[tokio::test]
async fn speech_start_interrupts_old_work_and_explicit_abort_suppresses_stale_output() {
    let mut fixture = Fixture::new();
    fixture.engine.agent_blocking.store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0, 8]),"format":"pcm","isLast":true}),
    );
    fixture.until("voice.transcription.result").await;
    let old = fixture.connection.voice.generation.clone();
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0, 8]),"format":"pcm","isLast":false}),
    );
    assert_ne!(fixture.connection.voice.generation, old);
    let result = fixture
        .connection
        .request("voice.abort.request", json!({}), &fixture.service)
        .await
        .unwrap();
    assert_eq!(result["accepted"], true);
    fixture.runtime.tasks.close();
    fixture.runtime.tasks.wait().await;
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 1);
    fixture.poll();
    assert!(
        !fixture
            .events
            .iter()
            .any(|event| event["method"] == "voice.audio.output")
    );
}

#[tokio::test]
async fn same_agent_cannot_be_owned_by_two_sockets_and_unavailable_backends_are_explicit() {
    let mut fixture = Fixture::new();
    fixture.mode(true).await;
    let mut second = Connection::default();
    let params = json!({"enabled":true,"agentId":"agent"});
    let denied = second
        .request("voice.mode.set.request", params.clone(), &fixture.service)
        .await
        .unwrap();
    assert_eq!(denied["accepted"], false);
    fixture.mode(false).await;
    assert_eq!(
        second
            .request("voice.mode.set.request", params, &fixture.service)
            .await
            .unwrap()["accepted"],
        true
    );
    fixture.service = Speech::new(None, None, None);
    assert_eq!(
        fixture.mode(true).await["reasonCode"],
        "speech_backend_unavailable"
    );
    fixture.start("unavailable");
    assert_eq!(
        fixture.until("dictation.stream.error").await["reasonCode"],
        "speech_backend_unavailable"
    );
}

#[tokio::test]
async fn disconnect_cancels_transcription_and_releases_all_process_permits() {
    let mut fixture = Fixture::new();
    fixture.engine.blocking.store(true, Ordering::SeqCst);
    fixture.start("stream");
    fixture.chunk("stream", 0, &[1, 0]);
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":0}),
    );
    for _ in 0..100 {
        if fixture.engine.calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    drop(fixture.connection);
    fixture.runtime.tasks.close();
    fixture.runtime.tasks.wait().await;
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.service.jobs.available_permits(), 4);
}

#[tokio::test]
async fn pcm_silence_finishes_an_utterance_without_explicit_last() {
    let mut fixture = Fixture::new();
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0, 8]),"format":"audio/pcm;rate=16000","isLast":false}),
    );
    fixture.event("voice.audio.chunk", json!({"audio":STANDARD.encode(vec![0; 19200]),"format":"audio/pcm;rate=16000","isLast":false}));
    fixture.until("voice.transcription.result").await;
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn unacknowledged_playback_expires_and_invalid_modes_preserve_current_target() {
    let mut fixture = Fixture::new();
    fixture.mode(true).await;
    let rejected = fixture
        .connection
        .request(
            "voice.mode.set.request",
            json!({"enabled":true,"agentId":"bad"}),
            &fixture.service,
        )
        .await
        .unwrap();
    assert_eq!(rejected["accepted"], false);
    assert_eq!(rejected["agentId"], "agent-1");
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0, 8]),"format":"pcm","isLast":true}),
    );
    fixture.until("voice.audio.output").await;
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        fixture.until("voice.error").await["reasonCode"],
        "speech_timeout"
    );
    assert!(fixture.connection.voice.acknowledgments.is_empty());
}

#[tokio::test(start_paused = true)]
async fn transcription_timeout_cancels_backend_and_delivers_error_before_releasing_capacity() {
    let mut fixture = Fixture::new();
    fixture.engine.blocking.store(true, Ordering::SeqCst);
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0, 8]),"format":"pcm","isLast":true}),
    );
    tokio::task::yield_now().await;
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(121)).await;
    fixture.runtime.tasks.close();
    fixture.runtime.tasks.wait().await;
    assert_eq!(
        fixture.until("voice.error").await["reasonCode"],
        "speech_timeout"
    );
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.service.jobs.available_permits(), 4);
    assert!(fixture.connection.is_empty());
}
