//! Discovery tolerates non-snapshot listings without weakening cursor validation.

use ait_agent_adapters::{AdapterError, codex::drive_thread_list_protocol};
use ait_ports::{CodexThreadSnapshot, CodexThreadSourceKind};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader, split};

use super::{client, read_json, thread, write_json};

async fn scan(
    pages: Vec<(bool, Option<&str>, Value)>,
) -> Result<Vec<CodexThreadSnapshot>, AdapterError> {
    let (client_io, server_io) = tokio::io::duplex(32 * 1024);
    let (client_read, client_write) = split(client_io);
    let (server_read, mut server_write) = split(server_io);
    let pages = pages
        .into_iter()
        .map(|(archived, cursor, page)| (archived, cursor.map(str::to_owned), page))
        .collect::<Vec<_>>();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        assert_eq!(read_json(&mut lines).await["method"], "initialize");
        write_json(&mut server_write, json!({"id": 0, "result": {}})).await;
        assert_eq!(read_json(&mut lines).await["method"], "initialized");
        for (archived, cursor, page) in pages {
            let request = read_json(&mut lines).await;
            assert_eq!(request["method"], "thread/list");
            assert_eq!(request["params"]["archived"], archived);
            assert_eq!(request["params"]["cursor"], json!(cursor));
            assert_eq!(request["params"]["sortKey"], "created_at");
            assert_eq!(request["params"]["sortDirection"], "asc");
            write_json(
                &mut server_write,
                json!({"id": request["id"], "result": page}),
            )
            .await;
        }
    });
    let result = drive_thread_list_protocol(
        client_read,
        client_write,
        client(),
        vec![CodexThreadSourceKind::Cli],
    )
    .await;
    server.await.unwrap();
    result
}

#[tokio::test]
async fn duplicate_rows_within_a_page_keep_one_latest_observation() {
    let original = thread("one");
    let mut changed = original.clone();
    changed["preview"] = json!("changed without a timestamp change");
    changed["name"] = json!("Latest name");
    let threads = scan(vec![
        (
            false,
            None,
            json!({"data": [original, changed, thread("two")], "nextCursor": null}),
        ),
        (true, None, json!({"data": [], "nextCursor": null})),
    ])
    .await
    .unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].id, "one");
    assert_eq!(threads[0].preview, "changed without a timestamp change");
    assert_eq!(threads[0].name.as_deref(), Some("Latest name"));
    assert_eq!(threads[1].id, "two");
}

#[tokio::test]
async fn overlapping_and_duplicate_only_pages_continue_to_later_unique_threads() {
    let first = thread("one");
    let mut changed = first.clone();
    changed["updatedAt"] = json!(1);
    changed["preview"] = json!("later observation with an older timestamp");
    let threads = scan(vec![
        (
            false,
            None,
            json!({"data": [first], "nextCursor": "page:2"}),
        ),
        (
            false,
            Some("page:2"),
            json!({"data": [changed], "nextCursor": "page:3"}),
        ),
        (
            false,
            Some("page:3"),
            json!({"data": [thread("two")], "nextCursor": null}),
        ),
        (true, None, json!({"data": [], "nextCursor": null})),
    ])
    .await
    .unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].updated_at, 1);
    assert_eq!(
        threads[0].preview,
        "later observation with an older timestamp"
    );
    assert_eq!(threads[1].id, "two");
}

#[tokio::test]
async fn thread_archived_during_scan_is_merged_using_the_later_archive_observation() {
    let mut archived = thread("moving");
    archived["name"] = json!("Archived name");
    archived["turns"] = json!([{"id":"summary-turn", "status":"completed", "items": []}]);
    archived["customMetadata"] = json!("preserved");
    let threads = scan(vec![
        (
            false,
            None,
            json!({"data": [thread("moving"), thread("active")], "nextCursor": null}),
        ),
        (
            true,
            None,
            json!({"data": [archived, thread("archive-only")], "nextCursor": null}),
        ),
    ])
    .await
    .unwrap();
    assert_eq!(threads.len(), 3);
    assert_eq!(threads[0].id, "moving");
    assert!(threads[0].archived);
    assert_eq!(threads[0].name.as_deref(), Some("Archived name"));
    assert!(threads[0].turns.is_empty());
    assert_eq!(threads[0].metadata["customMetadata"], "preserved");
    assert!(!threads[1].archived);
    assert!(threads[2].archived);
}

#[tokio::test]
async fn duplicate_threads_do_not_mask_a_cursor_cycle() {
    let error = scan(vec![
        (
            false,
            None,
            json!({"data": [thread("one")], "nextCursor": "loop"}),
        ),
        (
            false,
            Some("loop"),
            json!({"data": [thread("one")], "nextCursor": "loop"}),
        ),
    ])
    .await
    .unwrap_err();
    assert!(error.to_string().contains("repeated pagination cursor"));
}

#[tokio::test]
async fn invalid_identity_is_not_silently_coalesced() {
    let error = scan(vec![(
        false,
        None,
        json!({"data": [thread(" ")], "nextCursor": null}),
    )])
    .await
    .unwrap_err();
    assert!(error.to_string().contains("empty Thread id"));
}
