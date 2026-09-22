use super::{normalize_workspace_label_name, workspace_label_key};

#[test]
fn names_collapse_whitespace_and_compare_without_case() {
    assert_eq!(
        normalize_workspace_label_name("  Needs\t  review\n"),
        "Needs review"
    );
    assert_eq!(workspace_label_key(" Needs REVIEW "), "needs review");
}
