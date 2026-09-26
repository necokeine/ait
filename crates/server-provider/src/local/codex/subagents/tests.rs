use super::*;

fn spawn(sender: &str, receiver: &str) -> Value {
    json!({"threadId":sender,"turnId":"spawn-turn","item":{"id":"spawn","type":"collabAgentToolCall",
        "senderThreadId":sender,"receiverThreadIds":[receiver],"tool":"spawnAgent","status":"completed",
        "prompt":"inspect","agentsStates":{(receiver):{"status":"running"}}}})
}

#[test]
fn only_proven_descendants_can_stream_and_turns_do_not_end_their_parent() {
    let mut live = Live::default();
    let images = ImageStore::default();
    assert!(
        live.observe(
            "item/completed",
            &spawn("foreign", "bad"),
            ("root", "/tmp"),
            &images
        )
        .unwrap()
        .is_empty()
    );
    live.observe(
        "item/completed",
        &spawn("root", "child"),
        ("root", "/tmp"),
        &images,
    )
    .unwrap();
    live.observe(
        "item/completed",
        &spawn("child", "nested"),
        ("root", "/tmp"),
        &images,
    )
    .unwrap();
    assert_eq!(live.children()[1].parent_id, "child");
    let text = json!({"threadId":"nested","turnId":"reply","item":{"id":"answer","type":"agentMessage","text":"child response"}});
    let events = live
        .observe("item/completed", &text, ("root", "/tmp"), &images)
        .unwrap();
    assert!(events.iter().any(|event| matches!(event, AgentTurnEvent::Subagent(SubagentEvent::Timeline {id,entry}) if id == "nested" && entry.item["text"] == "child response")));
    assert!(
        live.observe("item/completed", &text, ("root", "/tmp"), &images)
            .unwrap()
            .is_empty()
    );
    live.observe(
        "turn/completed",
        &json!({"threadId":"nested","turn":{"id":"reply","status":"completed"}}),
        ("root", "/tmp"),
        &images,
    )
    .unwrap();
    assert_eq!(live.children()[1].descriptor["status"], "completed");
    assert_eq!(live.children()[0].descriptor["status"], "running");
    assert!(
        live.observe("item/completed", &text, ("root", "/tmp"), &images)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn early_child_output_waits_for_spawn_provenance_and_verified_children_resume() {
    let mut live = Live::default();
    let images = ImageStore::default();
    let mut started = spawn("root", "child");
    started["item"]["receiverThreadIds"] = json!([]);
    live.observe("item/started", &started, ("root", "/tmp"), &images)
        .unwrap();
    let output = json!({"threadId":"child","turnId":"child-turn","item":{"type":"agentMessage","id":"answer","text":"Early result"}});
    assert!(
        live.observe("item/completed", &output, ("root", "/tmp"), &images)
            .unwrap()
            .is_empty()
    );
    let events = live
        .observe(
            "item/completed",
            &spawn("root", "child"),
            ("root", "/tmp"),
            &images,
        )
        .unwrap();
    assert!(events.iter().any(|event|matches!(event,AgentTurnEvent::Subagent(SubagentEvent::Timeline {entry,..}) if entry.item["text"]=="Early result")));
    let mut restored = Live::restore(live.children(), "root").unwrap();
    assert!(restored.contains("child"));
    let announcement = json!({"thread":{"id":"child","parentThreadId":"root","cwd":"/tmp/child-worktree","createdAt":1_700_000_000,"updatedAt":1_700_000_000}});
    restored
        .observe("thread/started", &announcement, ("root", "/tmp"), &images)
        .unwrap();
    assert_eq!(restored.children()[0].cwd, "/tmp/child-worktree");
    restored.stopped();
    assert_eq!(restored.children()[0].descriptor["status"], "canceled");
    assert!(
        Live::restore(live.children(), "foreign")
            .unwrap()
            .children()
            .is_empty()
    );
}
