use super::*;

#[test]
fn custom_prompts_keep_frontmatter_quoted_arguments_and_literal_dollars() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("review.md"), "---\ndescription: 'Review changes'\nargument-hint: [FILE]\n---\n$1 | $2 | $FILE | $FILE_MORE | $$FILE | $ARGUMENTS").unwrap();
    let commands = list(root.path()).unwrap();
    assert_eq!(commands[0]["name"], "prompts:review");
    assert_eq!(commands[0]["description"], "Review changes");
    assert_eq!(commands[0]["argumentHint"], "[FILE]");
    assert_eq!(
        invoke(
            root.path(),
            "review",
            "'hello world' second FILE='a b' FILE_MORE=more"
        )
        .unwrap(),
        "hello world | second | a b | more | $FILE | 'hello world' second FILE='a b' FILE_MORE=more"
    );
    assert_eq!(
        expand("$MISSING $1 $$ $1", "'value\\nnext'").unwrap(),
        "$MISSING value\nnext $ value\nnext"
    );
    assert!(invoke(root.path(), "../outside", "").is_err());
    assert!(invoke(root.path(), "missing", "").is_err());
    assert!(expand(&"$1".repeat(32768), "long value").is_err());
    assert!(list(&root.path().join("missing")).unwrap().is_empty());
}
