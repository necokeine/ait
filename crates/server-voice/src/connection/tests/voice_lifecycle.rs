//! Rust buffered-utterance counterparts of Paseo voice-session and voice-turn-controller tests.
use super::*;

fn utterance(fixture: &mut Fixture) {
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0,8,0,8]),
        "format":"audio/pcm;rate=16000","isLast":true}),
    );
}

async fn idle(fixture: &mut Fixture) {
    fixture.runtime.tasks.close();
    fixture.runtime.tasks.wait().await;
    fixture.poll();
}

fn event_count(fixture: &Fixture, method: &str) -> usize {
    fixture
        .events
        .iter()
        .filter(|event| event["method"] == method)
        .count()
}

#[tokio::test]
async fn final_transcript_is_submitted_to_the_agent_exactly_once() {
    let mut fixture = Fixture::new();
    fixture.mode(true).await;
    utterance(&mut fixture);
    idle(&mut fixture).await;
    for _ in 0..10 {
        fixture.poll();
    }
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":"","format":"audio/pcm;rate=16000","isLast":true}),
    );
    fixture.poll();
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.engine.agent_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.engine.synthesis_calls.load(Ordering::SeqCst), 1);
    assert_eq!(event_count(&fixture, "voice.transcription.result"), 1);
}

#[tokio::test]
async fn empty_final_transcript_is_reported_without_submitting_an_agent_turn() {
    let mut fixture = Fixture::new();
    *fixture.engine.transcript.lock().unwrap() = Some(" \n ".into());
    fixture.mode(true).await;
    utterance(&mut fixture);
    idle(&mut fixture).await;
    let result = fixture.until("voice.transcription.result").await;
    assert_eq!(result["text"], " \n ");
    assert_eq!(fixture.engine.agent_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.engine.synthesis_calls.load(Ordering::SeqCst), 0);
    assert_eq!(event_count(&fixture, "voice.audio.output"), 0);
    assert!(!fixture.connection.voice.running);
}

#[tokio::test]
async fn transcription_only_mode_does_not_require_agent_or_synthesis_backends() {
    let mut fixture = Fixture::new();
    fixture.service = Speech::new(Some(fixture.engine.clone()), None, None);
    utterance(&mut fixture);
    idle(&mut fixture).await;
    assert_eq!(
        fixture.until("voice.transcription.result").await["text"],
        "recognized speech"
    );
    assert_eq!(fixture.engine.agent_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.engine.synthesis_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.connection.is_empty());
}

#[tokio::test]
async fn silence_only_chunks_do_not_interrupt_an_active_agent_turn() {
    let mut fixture = Fixture::new();
    fixture.engine.agent_blocking.store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    fixture.until("voice.transcription.result").await;
    let generation = fixture.connection.voice.generation.clone();
    fixture.event(
        "voice.audio.chunk",
        json!({"audio":STANDARD.encode([0,0,0,0]),
        "format":"audio/pcm;rate=16000","isLast":false}),
    );
    assert_eq!(fixture.connection.voice.generation, generation);
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.engine.agent_calls.load(Ordering::SeqCst), 1);
    fixture.mode(false).await;
    idle(&mut fixture).await;
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn abort_during_synthesis_suppresses_output_and_releases_job_capacity() {
    let mut fixture = Fixture::new();
    fixture
        .engine
        .synthesis_blocking
        .store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    fixture.until("voice.transcription.result").await;
    assert_eq!(fixture.engine.synthesis_calls.load(Ordering::SeqCst), 1);
    fixture
        .connection
        .request("voice.abort.request", json!({}), &fixture.service)
        .await
        .unwrap();
    idle(&mut fixture).await;
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(event_count(&fixture, "voice.audio.output"), 0);
    assert_eq!(event_count(&fixture, "voice.error"), 0);
    assert_eq!(fixture.service.jobs.available_permits(), 4);
}

#[tokio::test]
async fn disabling_mode_discards_audio_already_queued_by_the_synthesis_worker() {
    let mut fixture = Fixture::new();
    fixture.mode(true).await;
    utterance(&mut fixture);
    fixture.runtime.tasks.close();
    fixture.runtime.tasks.wait().await;
    fixture.mode(false).await;
    fixture.poll();
    assert_eq!(event_count(&fixture, "voice.audio.output"), 0);
    assert_eq!(event_count(&fixture, "voice.transcription.result"), 0);
    assert!(fixture.connection.is_empty());
    assert!(fixture.service.claim("agent-1".into()).is_ok());
}

#[tokio::test]
async fn agent_failure_reports_one_error_and_never_calls_synthesis() {
    let mut fixture = Fixture::new();
    fixture.engine.agent_failing.store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    idle(&mut fixture).await;
    assert_eq!(
        fixture.until("voice.error").await["reasonCode"],
        "voice_agent_failed"
    );
    assert_eq!(fixture.engine.agent_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.engine.synthesis_calls.load(Ordering::SeqCst), 0);
    assert_eq!(event_count(&fixture, "voice.error"), 0);
    assert!(!fixture.connection.voice.running);
    assert_eq!(fixture.service.jobs.available_permits(), 4);
}

#[tokio::test]
async fn synthesis_failure_preserves_the_transcript_and_releases_capacity() {
    let mut fixture = Fixture::new();
    fixture
        .engine
        .synthesis_failing
        .store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    idle(&mut fixture).await;
    assert_eq!(
        fixture.until("voice.transcription.result").await["text"],
        "recognized speech"
    );
    assert_eq!(
        fixture.until("voice.error").await["reasonCode"],
        "speech_provider_failed"
    );
    assert_eq!(event_count(&fixture, "voice.audio.output"), 0);
    assert_eq!(fixture.service.jobs.available_permits(), 4);
    assert!(!fixture.connection.voice.running);
}

#[tokio::test]
async fn blank_agent_reply_finishes_without_synthesizing_silence() {
    let mut fixture = Fixture::new();
    fixture.engine.empty_reply.store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    idle(&mut fixture).await;
    assert_eq!(fixture.engine.agent_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.engine.synthesis_calls.load(Ordering::SeqCst), 0);
    assert_eq!(event_count(&fixture, "voice.error"), 0);
    assert_eq!(event_count(&fixture, "voice.audio.output"), 0);
}

#[tokio::test]
async fn repeated_enable_preserves_active_agent_work_and_exclusive_ownership() {
    let mut fixture = Fixture::new();
    fixture.engine.agent_blocking.store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    fixture.until("voice.transcription.result").await;
    let generation = fixture.connection.voice.generation.clone();
    assert_eq!(fixture.mode(true).await["accepted"], true);
    assert_eq!(fixture.connection.voice.generation, generation);
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 0);
    assert!(fixture.service.claim("agent-1".into()).is_err());
    fixture.mode(false).await;
    idle(&mut fixture).await;
    assert!(fixture.service.claim("agent-1".into()).is_ok());
}

#[tokio::test(start_paused = true)]
async fn target_resolution_timeout_preserves_the_previous_mode_and_agent() {
    let mut fixture = Fixture::new();
    fixture.mode(true).await;
    fixture
        .engine
        .resolve_blocking
        .store(true, Ordering::SeqCst);
    let result = fixture.mode(true).await;
    assert_eq!(result["accepted"], false);
    assert_eq!(result["reasonCode"], "speech_timeout");
    assert_eq!(result["enabled"], true);
    assert_eq!(result["agentId"], "agent-1");
}

#[tokio::test]
async fn malformed_abort_does_not_cancel_the_current_turn() {
    let mut fixture = Fixture::new();
    fixture.engine.agent_blocking.store(true, Ordering::SeqCst);
    fixture.mode(true).await;
    utterance(&mut fixture);
    fixture.until("voice.transcription.result").await;
    let generation = fixture.connection.voice.generation.clone();
    assert_eq!(
        fixture
            .connection
            .request(
                "voice.abort.request",
                json!({"extra":true}),
                &fixture.service
            )
            .await
            .unwrap_err(),
        Error::Invalid
    );
    assert_eq!(fixture.connection.voice.generation, generation);
    assert_eq!(fixture.engine.cancelled.load(Ordering::SeqCst), 0);
    fixture.mode(false).await;
    idle(&mut fixture).await;
}
