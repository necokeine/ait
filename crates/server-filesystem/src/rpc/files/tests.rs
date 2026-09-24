use serde_json::json;

use super::*;

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

#[test]
fn file_observation_detects_transitions_and_suppresses_repeated_errors() {
    let mut observation = FileObservation::new(
        "/repo".to_owned(),
        "a".to_owned(),
        port::FileVersion::Missing,
    );
    assert!(observation.update(port::FileVersion::Missing).is_none());
    let next = observation
        .update(port::FileVersion::Error("denied".to_owned()))
        .unwrap();
    assert_eq!(serde_json::to_value(next).unwrap()["error"], "denied");
    assert!(
        observation
            .update(port::FileVersion::Error("denied".to_owned()))
            .is_none()
    );
    assert!(observation.update(port::FileVersion::Missing).is_some());
}

#[test]
fn binary_preview_enforces_limit_and_reports_wire_metadata() {
    use crate::local::files::LocalFiles;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a.txt"), "abc").unwrap();
    let files = Files::new(Box::new(LocalFiles::new(
        temp.path().to_path_buf(),
        temp.path(),
    )));
    let cwd = temp.path().to_str().unwrap();
    assert!(binary_preview(&files, cwd, "a.txt", Some(2)).is_err());
    let (metadata, cursor) = binary_preview(&files, cwd, "a.txt", Some(3)).unwrap();
    assert_eq!(metadata.size, 3);
    assert_eq!(metadata.encoding, "utf-8");
    assert_eq!(cursor.read_chunk().unwrap().1.unwrap(), b"abc");
}
