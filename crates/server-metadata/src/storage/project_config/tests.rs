use serde_json::json;

use super::*;

#[test]
fn reads_missing_and_existing_documents_and_rejects_invalid_json() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_str().unwrap();
    let store = LocalProjectConfigStore;
    assert_eq!(
        store.read(root).unwrap(),
        ProjectConfigDocument {
            config: None,
            revision: None
        }
    );
    std::fs::write(fixture.path().join(FILE_NAME), "{\"future\":true}").unwrap();
    let document = store.read(root).unwrap();
    assert_eq!(document.config, Some(json!({"future":true})));
    assert!(document.revision.is_some());
    std::fs::write(fixture.path().join(FILE_NAME), "invalid").unwrap();
    assert_eq!(store.read(root), Err(ProjectConfigStoreError::Invalid));
}

#[test]
fn writes_atomically_and_rejects_stale_or_absent_revision_mismatches() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_str().unwrap();
    let store = LocalProjectConfigStore;
    let first = store.write(root, &json!({"one":1}), None).unwrap();
    let ProjectConfigWrite::Written { revision, .. } = first else {
        panic!("expected write")
    };
    assert_eq!(
        std::fs::read_to_string(fixture.path().join(FILE_NAME)).unwrap(),
        "{\n  \"one\": 1\n}\n"
    );
    assert!(matches!(
        store.write(root, &json!({"two":2}), None).unwrap(),
        ProjectConfigWrite::Stale {
            current_revision: Some(_)
        }
    ));
    assert_eq!(store.read(root).unwrap().config, Some(json!({"one":1})));
    let second = store
        .write(root, &json!({"two":2}), Some(revision))
        .unwrap();
    assert!(matches!(second, ProjectConfigWrite::Written { .. }));
}
