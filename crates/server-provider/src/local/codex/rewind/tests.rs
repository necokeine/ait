use super::*;

#[cfg(unix)]
#[tokio::test]
async fn rewind_forks_exclusive_prefix_and_never_modifies_the_source() {
    let fixture = crate::test_support::Fixture::new();
    let original = json!([{"id":"t1","status":"completed","items":[{"id":"u1","type":"userMessage","content":[{"type":"text","text":"first"}]}]},
        {"id":"t2","status":"completed","items":[{"id":"u2","type":"userMessage","content":[{"type":"text","text":"second"}]}]}]);
    let path = fixture.cwd.join("native-history-source.json");
    std::fs::write(&path, original.to_string()).unwrap();
    let client = fixture.client();
    let history = client
        .native_rewind("source", &fixture.spec(), "u2")
        .await
        .unwrap();
    assert_eq!(history.entries.len(), 1);
    assert_ne!(history.descriptor.provider_handle_id, "source");
    let empty = client
        .native_rewind("source", &fixture.spec(), "u1")
        .await
        .unwrap();
    assert!(empty.entries.is_empty());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original.to_string()
    );
    assert!(
        client
            .native_rewind("source", &fixture.spec(), "absent")
            .await
            .is_err()
    );
    fixture.mode("fork-same-id");
    assert!(
        client
            .native_rewind("source", &fixture.spec(), "u2")
            .await
            .is_err()
    );
}
