use super::*;

fn call(id: &str, name: &str, input: Value) -> Value {
    let mut message =
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":name}]}});
    message["message"]["content"][0]["input"] = input;
    message
}

fn result(id: &str, result: Value) -> Value {
    let mut message =
        json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id}]}});
    message["toolUseResult"] = result;
    message
}

#[test]
fn task_tools_preserve_ids_status_updates_deletion_and_do_not_apply_failed_or_duplicate_results() {
    let mut tasks = Tasks::default();
    let legacy = call(
        "legacy",
        "TodoWrite",
        json!({"todos":[{"content":"Prior task","status":"in_progress","activeForm":"Working"}]}),
    );
    let snapshot = tasks.observe(&legacy).unwrap();
    assert_eq!(snapshot[0].1["items"][0]["id"], "legacy:0");
    assert!(tasks.observe(&legacy).unwrap().is_empty());
    tasks.observe(&call("list", "TaskList", json!({}))).unwrap();
    assert_eq!(
        tasks.observe(&result("list", json!({"tasks":[]}))).unwrap()[0].1["items"],
        json!([])
    );
    tasks
        .observe(&call(
            "create",
            "TaskCreate",
            json!({"subject":"Build","activeForm":"Building"}),
        ))
        .unwrap();
    let created = result("create", json!({"task":{"id":"1"}}));
    assert_eq!(
        tasks.observe(&created).unwrap()[0].1["items"][0]["text"],
        "Build"
    );
    assert!(tasks.observe(&created).unwrap().is_empty());
    tasks
        .observe(&call(
            "update",
            "TaskUpdate",
            json!({"taskId":"1","status":"completed"}),
        ))
        .unwrap();
    assert_eq!(
        tasks
            .observe(&result("update", json!({"success":true})))
            .unwrap()[0]
            .1["items"][0]["completed"],
        true
    );
    tasks
        .observe(&call(
            "failed",
            "TaskUpdate",
            json!({"taskId":"1","status":"deleted"}),
        ))
        .unwrap();
    assert!(
        tasks
            .observe(&result("failed", json!({"success":false})))
            .unwrap()
            .is_empty()
    );
    assert_eq!(tasks.items.len(), 1);
    tasks
        .observe(&call(
            "delete",
            "TaskUpdate",
            json!({"taskId":"1","status":"deleted"}),
        ))
        .unwrap();
    assert_eq!(
        tasks.observe(&result("delete", json!({}))).unwrap()[0].1["items"],
        json!([])
    );
}
