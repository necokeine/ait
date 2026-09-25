use super::*;
use crate::protocol::timeline::{Cursor, NativeItem};

fn row(seq: u64, item: Value) -> Row {
    Row {
        seq,
        provider: "codex".to_owned(),
        entry: NativeItem {
            key: seq.to_string(),
            turn_id: None,
            timestamp: "time".to_owned(),
            item,
        },
    }
}

fn request() -> ForkRequest {
    ForkRequest {
        agent_id: "agent".to_owned(),
        boundary_cursor: None,
        boundary_message_id: None,
    }
}

#[test]
fn attachments_use_inclusive_boundaries_and_omit_private_payloads() {
    let rows = vec![
        row(1, json!({"type":"user_message","text":"question"})),
        row(
            2,
            json!({"type":"assistant_message","text":"first","messageId":"reply"}),
        ),
        row(
            3,
            json!({"type":"assistant_message","text":"second","messageId":"reply"}),
        ),
        row(
            4,
            json!({"type":"tool_call","name":"Read","input":"private input"}),
        ),
        row(5, json!({"type":"reasoning","text":"private reasoning"})),
        row(6, json!({"type":"plugin","data":"private plugin"})),
    ];
    let agent = json!({"title":"Agent title","cwd":"/tmp"});
    let exported = export(&request(), "epoch", &rows, &agent).unwrap();
    let text = exported["attachment"]["text"].as_str().unwrap();
    assert!(text.contains("[Assistant] first\nsecond\n[Read]"));
    assert!(text.contains("Source agent: Agent title"));
    assert!(!text.contains("private"));
    assert_eq!(exported["itemCount"], 6);
    let mut boundary = request();
    boundary.boundary_message_id = Some(" reply ".to_owned());
    assert_eq!(
        export(&boundary, "epoch", &rows, &agent).unwrap()["itemCount"],
        3
    );
    boundary.boundary_cursor = Some(Cursor {
        epoch: "epoch".to_owned(),
        seq: 1,
    });
    assert_eq!(
        export(&boundary, "epoch", &rows, &agent).unwrap()["itemCount"],
        1
    );
    assert_eq!(
        export(&boundary, "stale", &rows, &agent),
        Err(ErrorCode::InvalidMessage)
    );
    boundary.boundary_cursor.as_mut().unwrap().seq = 99;
    assert!(export(&boundary, "epoch", &rows, &agent).is_err());
    boundary.boundary_cursor = None;
    boundary.boundary_message_id = Some("unknown".to_owned());
    assert!(export(&boundary, "epoch", &rows, &agent).is_err());
}

#[test]
fn empty_and_oversized_attachments_have_explicit_outcomes() {
    assert!(
        export(&request(), "epoch", &[], &json!({})).unwrap()["attachment"]["text"]
            .as_str()
            .unwrap()
            .contains("No chat history")
    );
    let rows = [row(
        1,
        json!({"type":"user_message","text":"x".repeat(512*1024)}),
    )];
    assert_eq!(
        export(&request(), "epoch", &rows, &json!({})),
        Err(ErrorCode::ResourceExhausted)
    );
}
