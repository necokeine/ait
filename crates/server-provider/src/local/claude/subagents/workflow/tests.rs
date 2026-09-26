use super::*;
use serde_json::json;

#[test]
fn native_workflow_results_unwrap_json_and_reject_missing_oversized_or_linked_output() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("workflow.output");
    std::fs::write(
        &output,
        json!({"result":{"report":{"body":"Final report"}}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        read(output.to_str().unwrap()).as_deref(),
        Some("Final report")
    );
    assert_eq!(format(&json!(false)).as_deref(), Some("false"));
    assert!(
        format(&json!({"a":1,"b":2}))
            .unwrap()
            .starts_with("```json")
    );
    assert!(
        format(&json!("界".repeat(100_000)))
            .unwrap()
            .ends_with("[Workflow output truncated]")
    );
    assert!(read("relative.output").is_none());
    #[cfg(unix)]
    {
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&output, &link).unwrap();
        assert!(read(link.to_str().unwrap()).is_none());
    }
    std::fs::write(&output, "x".repeat(1024 * 1024 + 1)).unwrap();
    assert!(read(output.to_str().unwrap()).is_none());
}
