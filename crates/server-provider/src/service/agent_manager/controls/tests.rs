use super::*;

fn child(id: &str, parent: &str) -> NativeSubagent {
    NativeSubagent {
        persistence: None,
        id: id.to_owned(),
        parent_id: parent.to_owned(),
        cwd: "/tmp".to_owned(),
        descriptor: json!({}),
    }
}

#[test]
fn descendants_isolates_roots_follows_nested_children_and_rejects_cycles() {
    let result = descendants(
        "root",
        vec![
            child("grandchild", "child"),
            child("foreign", "other"),
            child("child", "root"),
        ],
    )
    .unwrap();
    assert_eq!(
        result
            .iter()
            .map(|child| child.id.as_str())
            .collect::<Vec<_>>(),
        vec!["child", "grandchild"]
    );
    assert!(descendants("root", vec![child("child", "root"), child("root", "child")]).is_err());
}
