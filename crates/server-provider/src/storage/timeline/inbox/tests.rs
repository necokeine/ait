use super::*;

#[test]
fn retries_match_payload_and_policy_and_claimed_inputs_never_replay_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite3");
    let timeline = Timeline::open(&path).unwrap();
    let prompt = AgentPrompt::text("hello");
    assert_eq!(
        timeline.reserve_input("agent", "one", &prompt, "interrupt"),
        Ok(Receipt::New)
    );
    assert_eq!(
        timeline.reserve_input("agent", "one", &prompt, "interrupt"),
        Ok(Receipt::Accepted)
    );
    assert_eq!(
        timeline.reserve_input("agent", "one", &prompt, "steer"),
        Err(ErrorCode::IdempotencyConflict)
    );
    assert_eq!(
        timeline.reserve_input("agent", "one", &AgentPrompt::text("different"), "interrupt"),
        Err(ErrorCode::IdempotencyConflict)
    );
    timeline.claim_input("agent", "one").unwrap();
    assert_eq!(
        timeline.reserve_input("agent", "one", &prompt, "interrupt"),
        Ok(Receipt::Uncertain)
    );
    timeline
        .reserve_input("agent", "two", &prompt, "interrupt")
        .unwrap();
    drop(timeline);
    let timeline = Timeline::open(&path).unwrap();
    timeline.recover_inputs().unwrap();
    assert_eq!(
        timeline.reserve_input("agent", "one", &prompt, "interrupt"),
        Ok(Receipt::Uncertain)
    );
    let inputs = timeline.queued_inputs().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].message, "two");
    assert!(timeline.claim_input("agent", "one").is_err());
    timeline.claim_input("agent", "two").unwrap();
    timeline
        .finish_input("agent", "two", Receipt::Accepted)
        .unwrap();
    assert_eq!(
        timeline.reserve_input("agent", "two", &prompt, "interrupt"),
        Ok(Receipt::Accepted)
    );
    assert!(!timeline.has_queued_input("agent").unwrap());
}

#[test]
fn rejection_fallback_is_explicit_and_cancellation_never_replays_inputs() {
    let timeline = Timeline::memory().unwrap();
    let prompt = AgentPrompt::text("hello");
    timeline
        .reserve_input("agent", "one", &prompt, "steer")
        .unwrap();
    assert!(timeline.requeue_input("agent", "one").is_err());
    timeline.claim_input("agent", "one").unwrap();
    timeline.requeue_input("agent", "one").unwrap();
    assert!(timeline.has_queued_input("agent").unwrap());
    timeline.cancel_inputs("agent").unwrap();
    assert!(!timeline.has_queued_input("agent").unwrap());
    assert_eq!(
        timeline.reserve_input("agent", "one", &prompt, "steer"),
        Ok(Receipt::Rejected)
    );
    assert!(
        timeline
            .finish_input("agent", "one", Receipt::Accepted)
            .is_err()
    );
    assert!(
        timeline
            .finish_input("agent", "missing", Receipt::New)
            .is_err()
    );
}

#[test]
fn bounded_fifo_preserves_distinct_agents_and_rejects_excess_before_admission() {
    let timeline = Timeline::memory().unwrap();
    let prompt = AgentPrompt::text("hello");
    for index in 0..32 {
        timeline
            .reserve_input("agent", &index.to_string(), &prompt, "interrupt")
            .unwrap();
    }
    assert_eq!(
        timeline.reserve_input("agent", "overflow", &prompt, "interrupt"),
        Err(ErrorCode::ResourceExhausted)
    );
    timeline
        .reserve_input("other", "0", &prompt, "interrupt")
        .unwrap();
    let inputs = timeline.queued_inputs().unwrap();
    assert_eq!(inputs.len(), 33);
    assert_eq!(inputs[31].message, "31");
    assert_eq!(inputs[32].agent, "other");
    timeline.cancel_inputs("agent").unwrap();
    assert_eq!(timeline.queued_inputs().unwrap().len(), 1);
}
