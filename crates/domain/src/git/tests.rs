use super::*;

#[test]
fn git_commit_requires_a_full_lowercase_object_id() {
    let sha1 = GitCommit::parse("b".repeat(40)).unwrap();
    let sha256 = GitCommit::parse("c".repeat(64)).unwrap();

    assert!(sha1.is_valid());
    assert!(sha256.is_valid());
    assert!(serde_json::from_str::<GitCommit>("\"short\"").is_err());
    assert!(GitCommit::parse("A".repeat(40)).is_err());
}
