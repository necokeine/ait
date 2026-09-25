use super::*;
use crate::ports::native_history::SessionDescriptor;

#[test]
fn recent_query_matches_all_display_fields_and_paths_are_canonical_directories() {
    let descriptor = SessionDescriptor {
        provider_id: "codex".to_owned(),
        provider_label: "Codex".to_owned(),
        provider_handle_id: "Native".to_owned(),
        cwd: "/project".to_owned(),
        title: Some("Title".to_owned()),
        first_prompt_preview: Some("First".to_owned()),
        last_prompt_preview: Some("Last".to_owned()),
        last_activity_at: "2026-09-25T00:00:00Z".to_owned(),
    };
    for query in ["", "native", "project", "title", "first", "last"] {
        assert!(matches_query(&descriptor, query));
    }
    assert!(!matches_query(&descriptor, "absent"));
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        canonical(root.path().to_str().unwrap()).unwrap(),
        root.path().canonicalize().unwrap().to_str().unwrap()
    );
    assert!(canonical("relative").is_err());
    let file = root.path().join("file");
    std::fs::write(&file, b"file").unwrap();
    assert!(canonical(file.to_str().unwrap()).is_err());
}
