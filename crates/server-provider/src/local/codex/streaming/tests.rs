use super::*;

#[test]
fn only_known_steer_errors_prove_that_input_was_not_accepted() {
    for error in [
        json!({"code":-32601}),
        json!({"code":-32600,"message":"no active turn to steer"}),
        json!({"code":-32600,"message":"active turn uses a different output schema"}),
        json!({"code":-32600,"message":"expected active turn id `first` but found `second`"}),
        json!({"code":-32600,"data":{"codexErrorInfo":{"activeTurnNotSteerable":{}}}}),
    ] {
        assert!(steer_rejected(&error), "{error}");
    }
    for error in [
        json!({"code":-32603,"message":"no active turn to steer"}),
        json!({"code":-32600,"message":"unexpected invalid request"}),
        json!({"code":-32600}),
        json!({"message":"no active turn to steer"}),
        json!({"code":-32600,"message":"expected active turn id `` but found `second`"}),
        json!({"code":-32600,"message":"expected active turn id `one` but found `second` extra"}),
    ] {
        assert!(!steer_rejected(&error), "{error}");
    }
}

fn params(id: &str, delta: &str) -> Value {
    json!({"threadId":"thread","turnId":"turn","itemId":id,"delta":delta,"summaryIndex":0})
}

fn item(event: Option<AgentTurnEvent>) -> NativeItem {
    match event.unwrap() {
        AgentTurnEvent::Progress { entry, .. } => entry,
        other => panic!("{other:?}"),
    }
}

#[test]
fn text_deltas_preserve_identity_unicode_and_reasoning_boundaries() {
    let mut stream = Stream::default();
    let first = item(
        stream
            .progress("item/agentMessage/delta", &params("a", "你好"))
            .unwrap(),
    );
    assert_eq!(first.key, "native:turn:a");
    assert_eq!(
        first.item,
        json!({"type":"assistant_message","messageId":"a","text":"你好"})
    );
    assert!(
        stream
            .progress("item/agentMessage/delta", &params("a", ""))
            .unwrap()
            .is_none()
    );
    item(
        stream
            .progress("item/reasoning/summaryTextDelta", &params("r", "first"))
            .unwrap(),
    );
    let mut next = params("r", "second");
    next["summaryIndex"] = json!(1);
    assert_eq!(
        item(
            stream
                .progress("item/reasoning/summaryTextDelta", &next)
                .unwrap()
        )
        .item["text"],
        "\nsecond"
    );
    next["summaryIndex"] = json!(0);
    assert!(
        stream
            .progress("item/reasoning/summaryTextDelta", &next)
            .is_err()
    );
    assert!(stream.complete(&json!({"id":"a"})).unwrap());
    assert!(!stream.complete(&json!({"id":"a"})).unwrap());
    assert!(
        stream
            .progress("item/agentMessage/delta", &params("a", "late"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn tool_previews_are_bounded_and_stop_after_completion() {
    let mut stream = Stream::default();
    let started =
        json!({"turnId":"turn","item":{"type":"commandExecution","id":"tool","command":"echo"}});
    assert_eq!(
        item(stream.progress("item/started", &started).unwrap()).item["status"],
        "running"
    );
    assert!(stream.progress("item/started", &started).unwrap().is_none());
    let first = item(
        stream
            .progress("item/commandExecution/outputDelta", &params("tool", "one"))
            .unwrap(),
    );
    assert_eq!(first.item["detail"]["output"], "one");
    let last = item(
        stream
            .progress(
                "item/fileChange/outputDelta",
                &params("tool", &"界".repeat(MAX_OUTPUT)),
            )
            .unwrap(),
    );
    assert!(last.item["detail"]["output"].as_str().unwrap().len() <= MAX_OUTPUT);
    stream.complete(&json!({"id":"tool"})).unwrap();
    assert!(
        stream
            .progress("item/fileChange/outputDelta", &params("tool", "late"))
            .unwrap()
            .is_none()
    );
    assert!(stream.progress("ignored", &json!({})).unwrap().is_none());
}

#[test]
fn malformed_and_excessive_progress_is_rejected() {
    let mut stream = Stream::default();
    assert!(
        stream
            .progress("item/agentMessage/delta", &json!({}))
            .is_err()
    );
    assert!(
        stream
            .progress(
                "item/agentMessage/delta",
                &params("a", &"x".repeat(MAX_TEXT + 1))
            )
            .is_err()
    );
    assert!(stream.complete(&json!({})).is_err());
    for index in 0..MAX_ITEMS {
        stream.complete(&json!({"id":index.to_string()})).unwrap();
    }
    assert!(stream.complete(&json!({"id":"overflow"})).is_err());
}
