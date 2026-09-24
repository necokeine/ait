use serde_json::json;

use super::*;

#[test]
fn version_and_write_discriminators_preserve_omission_and_nulls() {
    let missing = FileVersion::Missing {
        cwd: "/repo".to_owned(),
        path: "a".to_owned(),
    };
    let result = serde_json::to_value(WriteResult {
        result: WriteOutcome::Conflict { version: missing },
    })
    .unwrap();
    assert_eq!(result["result"]["version"]["status"], "missing");
    let upload = UploadResult {
        file: None,
        error: Some("failed".to_owned()),
    };
    assert!(serde_json::to_value(upload).unwrap()["file"].is_null());
    let ready: FileVersion = serde_json::from_value(
        json!({"status":"ready","cwd":"/repo","path":"a","size":2,"modifiedAt":"today"}),
    )
    .unwrap();
    assert!(
        serde_json::to_value(ready)
            .unwrap()
            .get("revision")
            .is_none()
    );
}

#[test]
fn request_shapes_match_paseo() {
    let request: WriteRequest = serde_json::from_value(json!({"cwd":"/repo","path":"a","content":"hello","expectedModifiedAt":"today","expectedRevision":"rev"})).unwrap();
    assert_eq!(request.expected_revision.as_deref(), Some("rev"));
    assert!(
        serde_json::from_value::<WriteRequest>(json!({"cwd":"/repo","path":"a","content":"x"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<CreateRequest>(
            json!({"cwd":"/repo","parentPath":".","name":"x","kind":"link"})
        )
        .is_err()
    );
    let request: ExplorerRequest =
        serde_json::from_value(json!({"cwd":"/repo","mode":"file"})).unwrap();
    assert!(request.path.is_none());
}
