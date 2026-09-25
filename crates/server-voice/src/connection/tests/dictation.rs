use super::*;

#[tokio::test]
async fn out_of_order_finish_waits_for_missing_audio_and_replays_final_without_retranscription() {
    let mut fixture = Fixture::new();
    fixture.start("stream");
    assert_eq!(fixture.events.last().unwrap()["params"]["ackSeq"], -1);
    fixture.chunk("stream", 1, &[2, 0]);
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":1}),
    );
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 0);
    fixture.chunk("stream", 0, &[1, 0]);
    assert_eq!(fixture.events.last().unwrap()["params"]["ackSeq"], 1);
    let final_result = fixture.until("dictation.stream.final").await;
    assert_eq!(final_result["text"], "recognized speech");
    assert_eq!(fixture.engine.samples.lock().unwrap()[0], [1, 0, 2, 0]);
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":1}),
    );
    assert_eq!(fixture.until("dictation.stream.final").await, final_result);
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 1);
    let permits: Vec<_> = (0..16).map(|_| fixture.service.stream().unwrap()).collect();
    assert!(fixture.service.stream().is_err());
    drop(permits);
}

#[tokio::test]
async fn duplicate_chunks_are_idempotent_and_conflicts_cannot_corrupt_accepted_audio() {
    let mut fixture = Fixture::new();
    fixture.start("stream");
    fixture.chunk("stream", 0, &[1, 0]);
    fixture.chunk("stream", 0, &[1, 0]);
    fixture.chunk("stream", 0, &[9, 0]);
    assert_eq!(
        fixture.until("dictation.stream.error").await["reasonCode"],
        "invalid_audio_or_stream"
    );
    fixture.event(
        "dictation.stream.start",
        json!({"dictationId":"stream","format":"audio/wav"}),
    );
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":0}),
    );
    fixture.until("dictation.stream.final").await;
    assert_eq!(fixture.engine.samples.lock().unwrap()[0], [1, 0]);
}

#[tokio::test(start_paused = true)]
async fn unfinished_and_gapped_streams_expire_without_fabricated_results() {
    let mut fixture = Fixture::new();
    fixture.start("idle");
    fixture.start("gap");
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"gap","finalSeq":1}),
    );
    tokio::time::advance(Duration::from_secs(61)).await;
    fixture.poll();
    assert_eq!(
        fixture.until("dictation.stream.error").await["dictationId"],
        "idle"
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    fixture.poll();
    assert_eq!(
        fixture.until("dictation.stream.error").await["dictationId"],
        "gap"
    );
    assert!(fixture.connection.is_empty());
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelling_processing_and_reusing_id_ignores_stale_completion() {
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
    fixture.event("dictation.stream.cancel", json!({"dictationId":"stream"}));
    fixture.runtime.tasks.close();
    fixture.runtime.tasks.wait().await;
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 1);
    fixture.start("stream");
    fixture
        .connection
        .sender
        .send(Completion::Dictation {
            id: "stream".to_owned(),
            generation: "stale".to_owned(),
            final_result: true,
            result: Ok(Transcript::default()),
        })
        .await
        .unwrap();
    fixture.poll();
    assert!(
        !fixture
            .events
            .iter()
            .any(|event| event["method"] == "dictation.stream.final")
    );
}

#[tokio::test(start_paused = true)]
async fn partial_transcription_precedes_final_and_resource_limits_are_enforced() {
    let mut fixture = Fixture::new();
    fixture.start("stream");
    fixture.chunk("stream", 0, &vec![0; 32000]);
    tokio::time::advance(Duration::from_secs(3)).await;
    assert_eq!(
        fixture.until("dictation.stream.partial").await["text"],
        "recognized speech"
    );
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":0}),
    );
    fixture.until("dictation.stream.final").await;
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 2);
    for id in ["a", "b", "c", "d", "overflow"] {
        fixture.start(id);
    }
    assert_eq!(
        fixture.until("dictation.stream.error").await["reasonCode"],
        "speech_resource_exhausted"
    );
}

#[tokio::test]
async fn invalid_mutations_and_final_provider_failure_are_explicit() {
    let mut fixture = Fixture::new();
    fixture.start("stream");
    for params in [
        json!({"dictationId":"stream","seq":0,"audio":"AA==","format":"pcm"}),
        json!({"dictationId":"stream","seq":0,"audio":"AAABAA==","format":"audio/wav"}),
        json!({"dictationId":"stream","seq":129,"audio":"AAABAA==","format":"audio/pcm;rate=16000"}),
        json!({"dictationId":"stream","seq":-1,"audio":"AAABAA==","format":"pcm"}),
    ] {
        fixture.event("dictation.stream.chunk", params);
        assert_eq!(
            fixture.until("dictation.stream.error").await["retryable"],
            false
        );
    }
    fixture.chunk("stream", 1, &[1, 0]);
    fixture.chunk("stream", 1, &[2, 0]);
    fixture.until("dictation.stream.error").await;
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":0}),
    );
    fixture.until("dictation.stream.error").await;
    fixture.chunk("stream", 0, &[3, 0]);
    fixture.engine.failing.store(true, Ordering::SeqCst);
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":"stream","finalSeq":1}),
    );
    assert_eq!(
        fixture.until("dictation.stream.error").await["reasonCode"],
        "speech_provider_failed"
    );
    assert!(fixture.connection.is_empty());
}
