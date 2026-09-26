use super::*;
use crate::protocol::timeline::NativeItem;

mod paseo;

fn rows() -> Vec<Row> {
    (1..=405).map(|seq| Row { seq,provider:"codex".to_owned(),entry:NativeItem {key:seq.to_string(),turn_id:None,timestamp:"2026-09-25T00:00:00Z".to_owned(),item:json!({"type":if seq%2==0 {"assistant_message"} else {"user_message"},"text":"Hello\n WORLD"})}}).collect()
}
fn request(value: Value) -> FetchRequest {
    serde_json::from_value(value).unwrap()
}

#[test]
fn pages_have_exclusive_boundaries_and_stale_cursors_reset() {
    let rows = rows();
    let page = fetch(
        &request(json!({"agentId":"a","limit":2})),
        "epoch",
        &rows,
        &Value::Null,
    )
    .unwrap();
    assert_eq!(page["entries"][0]["seqStart"], 404);
    assert_eq!(page["hasOlder"], true);
    let before=fetch(&request(json!({"agentId":"a","direction":"before","cursor":{"epoch":"epoch","seq":3},"limit":0})),"epoch",&rows,&Value::Null).unwrap();
    assert_eq!(before["entries"].as_array().unwrap().len(), 2);
    let after = fetch(
        &request(json!({"agentId":"a","cursor":{"epoch":"epoch","seq":404}})),
        "epoch",
        &rows,
        &Value::Null,
    )
    .unwrap();
    assert_eq!(after["entries"][0]["seqStart"], 405);
    let stale = fetch(
        &request(
            json!({"agentId":"a","cursor":{"epoch":"old","seq":1},"limit":1,"mergeWindow":true}),
        ),
        "epoch",
        &rows,
        &Value::Null,
    )
    .unwrap();
    assert_eq!(stale["reset"], true);
    assert_eq!(stale["staleCursor"], true);
    assert_eq!(stale["mergeWindow"], true);
    assert!(
        fetch(
            &request(json!({"agentId":"a","direction":"before"})),
            "epoch",
            &rows,
            &Value::Null
        )
        .is_err()
    );
    let empty = fetch(&request(json!({"agentId":"a"})), "empty", &[], &Value::Null).unwrap();
    assert!(empty["startCursor"].is_null());
}

#[test]
fn search_paginates_by_sequence_and_prompts_exclude_assistant_rows() {
    let rows = rows();
    let query = SearchRequest {
        agent_id: "a".to_owned(),
        query: "HELLO world".to_owned(),
        cursor: None,
    };
    let first = search(&query, "epoch", &rows).unwrap();
    assert_eq!(first["locations"].as_array().unwrap().len(), 200);
    assert_eq!(first["nextCursor"], 200);
    let next = search(
        &SearchRequest {
            cursor: Some(200),
            ..query
        },
        "epoch",
        &rows,
    )
    .unwrap();
    assert_eq!(next["locations"][0]["seq"], 201);
    let list = prompts("a", "epoch", &rows).unwrap();
    assert_eq!(list["prompts"].as_array().unwrap().len(), 203);
    assert_eq!(list["prompts"][0]["preview"], "Hello WORLD");
    assert_eq!(
        bounded(json!("x".repeat(1024 * 1024))),
        Err(ErrorCode::ResourceExhausted)
    );
}

#[test]
fn prompt_previews_follow_paseo_length_and_keep_unicode_valid() {
    assert_eq!(preview("  short\n prompt  "), "short prompt");
    assert_eq!(preview(&"x".repeat(121)), format!("{}…", "x".repeat(119)));
    assert!(preview(&"😀".repeat(80)).encode_utf16().count() <= 120);
}
