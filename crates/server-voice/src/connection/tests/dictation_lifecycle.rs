//! Buffered equivalents of Paseo dictation-stream-manager commit, tail and cleanup tests.
use super::*;
use tokio::sync::oneshot;

struct Call {
    audio: Audio,
    reply: oneshot::Sender<Result<Transcript, Error>>,
}

#[derive(Debug)]
struct Gated(mpsc::UnboundedSender<Call>);

impl Transcriber for Gated {
    fn transcribe(&self, audio: Audio, cancel: CancellationToken) -> Operation<'_, Transcript> {
        Box::pin(async move {
            let (reply, receive) = oneshot::channel();
            self.0
                .send(Call { audio, reply })
                .unwrap_or_else(|_| panic!("test receiver remains alive"));
            tokio::select! {
                result = receive => result.expect("test must settle transcription"),
                () = cancel.cancelled() => Err(Error::Cancelled),
            }
        })
    }
}

fn gated(fixture: &mut Fixture) -> mpsc::UnboundedReceiver<Call> {
    let (sender, receiver) = mpsc::unbounded_channel();
    fixture.service = Speech::new(Some(Arc::new(Gated(sender))), None, None);
    receiver
}

async fn next_call(fixture: &mut Fixture, receiver: &mut mpsc::UnboundedReceiver<Call>) -> Call {
    for _ in 0..100 {
        fixture.poll();
        if let Ok(call) = receiver.try_recv() {
            return call;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("transcription was not admitted");
}

fn finish(fixture: &mut Fixture, id: &str, final_seq: u32) {
    fixture.event(
        "dictation.stream.finish",
        json!({"dictationId":id,"finalSeq":final_seq}),
    );
}

#[tokio::test(start_paused = true)]
async fn finalization_waits_for_inflight_partial_and_includes_audio_appended_during_it() {
    let mut fixture = Fixture::new();
    let mut calls = gated(&mut fixture);
    fixture.start("stream");
    fixture.chunk("stream", 0, &vec![1; 32_000]);
    tokio::time::advance(Duration::from_secs(2)).await;
    let partial = next_call(&mut fixture, &mut calls).await;
    assert_eq!(partial.audio.bytes.len(), 32_000);
    fixture.chunk("stream", 1, &[2, 0]);
    finish(&mut fixture, "stream", 1);
    assert!(calls.try_recv().is_err());
    partial
        .reply
        .send(Ok(Transcript {
            text: "obsolete partial".into(),
            language: None,
        }))
        .unwrap();
    let final_call = next_call(&mut fixture, &mut calls).await;
    assert_eq!(final_call.audio.bytes.len(), 32_002);
    assert_eq!(&final_call.audio.bytes[32_000..], &[2, 0]);
    assert!(
        !fixture
            .events
            .iter()
            .any(|event| event["method"] == "dictation.stream.partial")
    );
    final_call
        .reply
        .send(Ok(Transcript {
            text: "complete transcript".into(),
            language: None,
        }))
        .unwrap();
    assert_eq!(
        fixture.until("dictation.stream.final").await["text"],
        "complete transcript"
    );
    assert!(calls.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn failed_partial_keeps_audio_for_a_successful_final_transcription() {
    let mut fixture = Fixture::new();
    let mut calls = gated(&mut fixture);
    fixture.start("stream");
    fixture.chunk("stream", 0, &vec![3; 32_000]);
    tokio::time::advance(Duration::from_secs(2)).await;
    let partial = next_call(&mut fixture, &mut calls).await;
    partial.reply.send(Err(Error::Provider)).unwrap();
    tokio::task::yield_now().await;
    fixture.poll();
    assert!(
        !fixture
            .events
            .iter()
            .any(|event| event["method"] == "dictation.stream.error")
    );
    finish(&mut fixture, "stream", 0);
    let final_call = next_call(&mut fixture, &mut calls).await;
    assert_eq!(final_call.audio.bytes, vec![3; 32_000]);
    final_call
        .reply
        .send(Ok(Transcript {
            text: "recovered".into(),
            language: None,
        }))
        .unwrap();
    assert_eq!(
        fixture.until("dictation.stream.final").await["text"],
        "recovered"
    );
}

#[tokio::test]
async fn repeated_finish_while_processing_does_not_start_another_transcription() {
    let mut fixture = Fixture::new();
    let mut calls = gated(&mut fixture);
    fixture.start("stream");
    fixture.chunk("stream", 0, &[1, 0]);
    finish(&mut fixture, "stream", 0);
    let call = next_call(&mut fixture, &mut calls).await;
    for _ in 0..8 {
        finish(&mut fixture, "stream", 0);
    }
    assert!(calls.try_recv().is_err());
    call.reply
        .send(Ok(Transcript {
            text: "once".into(),
            language: None,
        }))
        .unwrap();
    assert_eq!(
        fixture.until("dictation.stream.final").await["text"],
        "once"
    );
    assert!(calls.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn repeated_finish_does_not_extend_the_original_gap_deadline() {
    let mut fixture = Fixture::new();
    fixture.start("stream");
    finish(&mut fixture, "stream", 1);
    tokio::time::advance(Duration::from_secs(119)).await;
    finish(&mut fixture, "stream", 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        fixture.until("dictation.stream.error").await["reasonCode"],
        "speech_timeout"
    );
    assert!(fixture.connection.is_empty());
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn completed_results_expire_without_a_spurious_stream_error() {
    let mut fixture = Fixture::new();
    fixture.start("stream");
    fixture.chunk("stream", 0, &[1, 0]);
    finish(&mut fixture, "stream", 0);
    fixture.until("dictation.stream.final").await;
    tokio::time::advance(Duration::from_secs(61)).await;
    fixture.poll();
    assert!(fixture.connection.is_empty());
    assert!(
        !fixture
            .events
            .iter()
            .any(|event| event["method"] == "dictation.stream.error")
    );
    fixture.start("stream");
    fixture.chunk("stream", 0, &[2, 0]);
    finish(&mut fixture, "stream", 0);
    fixture.until("dictation.stream.final").await;
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn completed_replay_cache_evicts_oldest_without_retaining_stream_permits() {
    let mut fixture = Fixture::new();
    for index in 0..33 {
        let id = format!("stream-{index}");
        fixture.start(&id);
        fixture.chunk(&id, 0, &[1, 0]);
        finish(&mut fixture, &id, 0);
        fixture.until("dictation.stream.final").await;
    }
    assert_eq!(fixture.connection.dictations.len(), 32);
    assert!(!fixture.connection.dictations.contains_key("stream-0"));
    assert!(fixture.connection.dictations.contains_key("stream-32"));
    let permits: Vec<_> = (0..16).map(|_| fixture.service.stream().unwrap()).collect();
    assert!(fixture.service.stream().is_err());
    drop(permits);
}

#[tokio::test]
async fn process_stream_budget_is_shared_across_connections_and_released_on_disconnect() {
    let mut fixtures: Vec<_> = (0..5).map(|_| Fixture::new()).collect();
    let shared = fixtures[0].service.clone();
    for fixture in &mut fixtures {
        fixture.service = shared.clone();
    }
    for (index, fixture) in fixtures.iter_mut().enumerate().take(4) {
        for stream in 0..4 {
            fixture.start(&format!("{index}-{stream}"));
        }
    }
    fixtures[4].start("overflow");
    assert_eq!(
        fixtures[4].until("dictation.stream.error").await["reasonCode"],
        "speech_resource_exhausted"
    );
    let released = fixtures.remove(0);
    drop(released);
    fixtures[3].start("replacement");
    assert!(
        fixtures[3]
            .connection
            .dictations
            .contains_key("replacement")
    );
}

#[tokio::test]
async fn dictation_waits_for_process_capacity_without_discarding_accepted_audio() {
    let mut fixture = Fixture::new();
    let permit = fixture
        .service
        .jobs
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    fixture.start("stream");
    fixture.chunk("stream", 0, &[1, 0]);
    finish(&mut fixture, "stream", 0);
    fixture.poll();
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 0);
    drop(permit);
    fixture.until("dictation.stream.final").await;
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.engine.samples.lock().unwrap()[0], [1, 0]);
}

#[tokio::test]
async fn cancelling_one_stream_preserves_another_stream_and_its_buffered_audio() {
    let mut fixture = Fixture::new();
    fixture.start("cancelled");
    fixture.start("kept");
    fixture.chunk("cancelled", 0, &[1, 0]);
    fixture.chunk("kept", 0, &[2, 0]);
    fixture.event(
        "dictation.stream.cancel",
        json!({"dictationId":"cancelled"}),
    );
    fixture.event(
        "dictation.stream.cancel",
        json!({"dictationId":"cancelled"}),
    );
    assert!(!fixture.connection.dictations.contains_key("cancelled"));
    finish(&mut fixture, "kept", 0);
    assert_eq!(
        fixture.until("dictation.stream.final").await["dictationId"],
        "kept"
    );
    assert_eq!(fixture.engine.samples.lock().unwrap()[0], [2, 0]);
}
