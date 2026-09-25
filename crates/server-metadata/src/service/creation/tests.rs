use super::*;

#[test]
fn creation_replays_committed_results_without_repeating_effects_and_rejects_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("receipts.json");
    let creations = Creations::open(path.clone()).unwrap();
    let intent = json!({"config":{"provider":"codex"}});
    let admitted = creations.begin(Kind::Agent, "key", intent.clone()).unwrap();
    assert!(admitted.execute);
    assert!(admitted.snapshot.agent_id.is_some());
    let completed = creations
        .advance(
            &admitted.snapshot,
            "completed",
            Some(json!({"agent":{"id":admitted.snapshot.agent_id}})),
            None,
        )
        .unwrap();
    assert_eq!(completed.revision, 1);
    assert!(matches!(
        creations.begin(Kind::Agent, "key", json!({"changed":true})),
        Err(ErrorCode::IdempotencyConflict)
    ));
    drop(creations);
    let creations = Creations::open(path).unwrap();
    let replay = creations.begin(Kind::Agent, "key", intent).unwrap();
    assert!(!replay.execute);
    assert_eq!(replay.snapshot, completed);
    assert!(
        creations
            .advance(&completed, "completed", None, None)
            .is_err()
    );
    assert!(
        creations
            .snapshot(Kind::Workspace, "key")
            .unwrap()
            .is_none()
    );
}

#[test]
fn interrupted_creation_is_observable_without_launching_a_second_attempt() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("receipts.json");
    let creations = Creations::open(path.clone()).unwrap();
    let admitted = creations.begin(Kind::Workspace, "key", json!({})).unwrap();
    assert!(
        !creations
            .snapshot(Kind::Workspace, "key")
            .unwrap()
            .unwrap()
            .outcome_unknown
    );
    drop(creations);
    let creations = Creations::open(path).unwrap();
    let snapshot = creations.snapshot(Kind::Workspace, "key").unwrap().unwrap();
    assert!(snapshot.outcome_unknown);
    assert_eq!(snapshot.workspace_id, admitted.snapshot.workspace_id);
    assert!(
        !creations
            .begin(Kind::Workspace, "key", json!({}))
            .unwrap()
            .execute
    );
}

#[test]
fn observer_can_arrive_before_creation_and_releases_without_affecting_work() {
    let creations = Creations::default();
    let (outbound, mut receiver) = Outbound::new();
    let (snapshot, subscription) = creations.subscribe(Kind::Agent, "key", outbound).unwrap();
    assert!(snapshot.is_none());
    let admitted = creations.begin(Kind::Agent, "key", json!({})).unwrap();
    assert!(receiver.try_recv().is_err());
    subscription.activate().unwrap();
    assert!(receiver.try_recv().is_ok());
    drop(subscription);
    creations
        .advance(
            &admitted.snapshot,
            "failed",
            None,
            Some("failed".to_owned()),
        )
        .unwrap();
    assert!(receiver.try_recv().is_err());
    assert!(validate_key("").is_err());
    assert!(validate_key(&"x".repeat(513)).is_err());
    assert!(creations.begin(Kind::Agent, "\n", json!({})).is_err());
    let root = tempfile::tempdir().unwrap();
    let bad = root.path().join("bad");
    std::fs::write(&bad, "not json").unwrap();
    assert!(Creations::open(bad).is_err());
}
