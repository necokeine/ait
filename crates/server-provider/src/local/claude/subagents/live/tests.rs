use super::*;

#[test]
fn only_workflows_read_native_output_and_duplicate_completion_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("result.output");
    std::fs::write(
        &path,
        json!({"result":{"report":"Workflow report"}}).to_string(),
    )
    .unwrap();
    let mut live = Live::new(ImageStore::default());
    live.observe(&json!({"type":"system","subtype":"task_started","task_type":"local_workflow","tool_use_id":"wf-call","task_id":"wf_1"}),"root","/tmp").unwrap();
    let notification = json!({"type":"system","subtype":"task_notification","task_id":"wf_1","status":"completed","output_file":path});
    let events = live.observe(&notification, "root", "/tmp").unwrap();
    assert!(events.iter().any(|event|matches!(event,AgentTurnEvent::Subagent(SubagentEvent::Timeline {entry,..}) if entry.item["text"]=="Workflow report")));
    assert!(
        live.observe(&notification, "root", "/tmp")
            .unwrap()
            .is_empty()
    );
    std::fs::write(&path, json!({"result":"Updated report"}).to_string()).unwrap();
    let updated = live.observe(&notification, "root", "/tmp").unwrap();
    assert!(updated.iter().any(|event|matches!(event,AgentTurnEvent::Subagent(SubagentEvent::Timeline {entry,..}) if entry.item["text"]=="Updated report")));
    live.observe(&json!({"type":"system","subtype":"task_started","task_type":"local_agent","tool_use_id":"agent-call","task_id":"agent_1"}),"root","/tmp").unwrap();
    let mut notification = notification;
    notification["task_id"] = json!("agent_1");
    assert!(
        !live
            .observe(&notification, "root", "/tmp")
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                AgentTurnEvent::Subagent(SubagentEvent::Timeline { .. })
            ))
    );
}

fn start(task: &str, call: &str) -> Value {
    json!({"type":"system","subtype":"task_started","task_id":task,"tool_use_id":call,
        "task_type":"local_agent","subagent_type":"Explore","prompt":"inspect"})
}

#[test]
fn announcements_aliases_nested_frames_and_background_lifetimes_are_separate() {
    let mut live = Live::new(ImageStore::default());
    let events = live
        .observe(&start("task", "call"), "root", "/tmp")
        .unwrap();
    assert!(matches!(
        events[0],
        AgentTurnEvent::Subagent(SubagentEvent::Upsert(_))
    ));
    live.observe(&json!({"type":"assistant","uuid":"child-msg","parent_tool_use_id":"call",
        "message":{"id":"assistant","content":[{"type":"tool_use","id":"nested","name":"Agent","input":{"name":"Verifier","prompt":"verify"}}]}}), "root", "/tmp").unwrap();
    live.observe(&start("nested-task", "nested"), "root", "/tmp")
        .unwrap();
    assert_eq!(live.children()[1].parent_id, "call");
    live.observe(&start("task", "alias"), "root", "/tmp")
        .unwrap();
    assert_eq!(live.children().len(), 2);
    let frames = live.observe(&json!({"type":"assistant","uuid":"answer","parent_tool_use_id":"alias",
        "message":{"id":"reply","model":"claude-child","content":[{"type":"text","text":"child answer"}]}}), "root", "/tmp").unwrap();
    assert!(frames.iter().any(|event| matches!(event, AgentTurnEvent::Subagent(SubagentEvent::Timeline { id, entry }) if id == "call" && entry.item["text"] == "child answer")));
    live.observe(&json!({"type":"system","subtype":"task_updated","task_id":"task","patch":{"is_backgrounded":true}}), "root", "/tmp").unwrap();
    live.stopped(false, false);
    assert_eq!(live.children()[0].descriptor["status"], "running");
    assert_eq!(live.children()[1].descriptor["status"], "canceled");
    live.observe(&json!({"type":"system","subtype":"task_notification","task_id":"task","status":"completed","usage":{"total_tokens":123}}), "root", "/tmp").unwrap();
    assert_eq!(live.children()[0].descriptor["status"], "completed");
    assert_eq!(live.children()[0].descriptor["subtitle"], "123 tokens");
}

#[test]
fn native_task_filters_prevent_ambient_shell_and_foreign_frames_from_becoming_children() {
    let mut live = Live::new(ImageStore::default());
    for mut record in [
        start("shell", "shell-call"),
        start("ambient", "ambient-call"),
    ] {
        if record["task_id"] == "shell" {
            record["task_type"] = json!("local_bash");
        } else {
            record["skip_transcript"] = json!(true);
        }
        assert!(live.observe(&record, "root", "/tmp").unwrap().is_empty());
    }
    assert!(
        live.observe(
            &json!({"type":"assistant","parent_tool_use_id":"shell-call"}),
            "root",
            "/tmp"
        )
        .unwrap()
        .is_empty()
    );
    assert!(live.observe(&json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed"}), "root", "/tmp").unwrap().is_empty());
    assert!(live.children().is_empty());
}
