use super::*;
use serde_json::json;

#[test]
fn projects_missing_and_failed_versions_without_inventing_content() {
    let missing = encode(project_version("/repo", "a", port::FileVersion::Missing)).unwrap();
    assert_eq!(
        missing,
        json!({"status":"missing","cwd":"/repo","path":"a"})
    );
    let error = encode(project_version(
        "/repo",
        "a",
        port::FileVersion::Error("denied".to_owned()),
    ))
    .unwrap();
    assert_eq!(error["error"], "denied");
}

#[test]
fn invalid_payloads_and_operation_failures_are_distinct() {
    assert!(matches!(
        decode::<wire::WriteRequest>(json!({"cwd":"x"})),
        Err(ErrorCode::InvalidMessage)
    ));
    let (path, error) = split::<String>(Err(port::FileError("missing".to_owned())));
    assert!(path.is_none());
    assert_eq!(error.as_deref(), Some("missing"));
}
