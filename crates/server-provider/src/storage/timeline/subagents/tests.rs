use super::*;
use serde_json::json;

#[test]
fn native_task_state_survives_restart_without_reparenting_or_rewriting_its_history() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.db");
    let timeline = Timeline::open(&path).unwrap();
    let mut child = NativeSubagent {
        persistence: None,
        id: "child".into(),
        parent_id: "native-root".into(),
        cwd: "/tmp".into(),
        descriptor: json!({"id":"child","status":"running"}),
    };
    timeline.store_subagent("host", &child).unwrap();
    child.descriptor["status"] = json!("completed");
    timeline.store_subagent("host", &child).unwrap();
    drop(timeline);
    let timeline = Timeline::open(&path).unwrap();
    assert_eq!(timeline.subagents("host").unwrap(), vec![child.clone()]);
    assert!(timeline.subagents("other").unwrap().is_empty());
    child.parent_id = "foreign".into();
    assert_eq!(
        timeline.store_subagent("host", &child),
        Err(ErrorCode::IdempotencyConflict)
    );
    assert_eq!(
        timeline.subagents("host").unwrap()[0].parent_id,
        "native-root"
    );
}
