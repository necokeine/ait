//! Repository-scale inspection through the real host tools.
#![allow(clippy::pedantic)]
use ait_domain::{RunId, RunPermissionProfile, ToolExecutionId};
use ait_ports::{RunToolFactory, ToolInvocation};
use ait_tools::host::{HostToolFactory, MAX_BYTES};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn call(name: &str, arguments: Value) -> ToolInvocation {
    ToolInvocation {
        run_id: RunId::new("run"),
        call_id: "call".into(),
        execution_id: ToolExecutionId::new("execution"),
        tool_name: name.into(),
        arguments,
        message_path: Vec::new(),
        cancellation: CancellationToken::new(),
    }
}

#[tokio::test]
async fn browses_scopes_counts_and_pages_without_loading_whole_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    std::fs::create_dir_all(path.join("crates/one/src")).unwrap();
    std::fs::create_dir_all(path.join("crates/two")).unwrap();
    std::fs::create_dir_all(path.join("target")).unwrap();
    std::fs::write(
        path.join("crates/one/src/large.rs"),
        "pub fn example() {}\n".repeat(10_000),
    )
    .unwrap();
    std::fs::write(path.join("crates/one/src/small.rs"), "a\nb\n").unwrap();
    std::fs::write(path.join("crates/two/other.rs"), "other\n").unwrap();
    std::fs::write(path.join("target/ignored.rs"), "build artifact\n").unwrap();
    let tools = HostToolFactory
        .create(&path, RunPermissionProfile::default())
        .unwrap();
    let directory = tools
        .execute(call("read", json!({"file_path":"crates"})))
        .await
        .unwrap()
        .output;
    assert_eq!(directory["entries"], json!(["one/", "two/"]));
    let lines = tools
        .execute(call(
            "read",
            json!({"file_path":"crates/one/src/large.rs","offset":9999,"limit":1}),
        ))
        .await
        .unwrap()
        .output;
    assert_eq!(lines["text"], "9999: pub fn example() {}\n");
    assert_eq!(lines["truncated"], true);
    assert_eq!(lines["next_offset"], 10_000);
    let counts = tools
        .execute(call(
            "grep",
            json!({"pattern":"^","path":"crates/one","include":"*.rs","output_mode":"count"}),
        ))
        .await
        .unwrap()
        .output;
    assert_eq!(counts["total_count"], 10_002);
    assert_eq!(counts["count_complete"], true);
    assert_eq!(counts["matches"][0]["count"], 10_000);
    assert_eq!(counts["matches"][1]["count"], 2);
    assert!(counts.to_string().len() < 1000);
    let first = tools
        .execute(call(
            "grep",
            json!({"pattern":"^","path":"crates/one/src/large.rs","limit":2}),
        ))
        .await
        .unwrap()
        .output;
    assert_eq!(first["matches"].as_array().unwrap().len(), 2);
    assert_eq!(first["total_count"], 10_000);
    assert_eq!(first["truncated"], true);
    let second = tools.execute(call("grep",json!({"pattern":"^","path":"crates/one/src/large.rs","offset":first["next_offset"],"limit":2}))).await.unwrap().output;
    assert_eq!(second["matches"][0]["line"], 3);
    let found = tools
        .execute(call("glob", json!({"pattern":"*.rs","path":"crates/one"})))
        .await
        .unwrap()
        .output;
    assert_eq!(found["matches"].as_array().unwrap().len(), 2);
    let all = tools
        .execute(call("grep", json!({"pattern":"^","output_mode":"count"})))
        .await
        .unwrap()
        .output;
    assert_eq!(all["total_count"], 10_003);
    assert!(all.to_string().len() < MAX_BYTES);
}

#[tokio::test]
async fn reports_partial_scans_and_rejects_scope_escape() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    std::fs::write(path.join("binary"), b"\0\0").unwrap();
    std::fs::write(path.join("large-line"), "x".repeat(MAX_BYTES + 1)).unwrap();
    std::fs::write(path.join("file"), "hello\n").unwrap();
    let tools = HostToolFactory
        .create(&path, RunPermissionProfile::default())
        .unwrap();
    let result = tools
        .execute(call("grep", json!({"pattern":"^","output_mode":"count"})))
        .await
        .unwrap()
        .output;
    assert_eq!(result["skipped_files"], 2);
    assert_eq!(result["count_complete"], false);
    assert_eq!(result["truncated"], true);
    for tool in ["grep", "glob"] {
        assert!(
            tools
                .execute(call(tool, json!({"pattern":"x","path":"../"})))
                .await
                .is_err()
        );
    }
}
