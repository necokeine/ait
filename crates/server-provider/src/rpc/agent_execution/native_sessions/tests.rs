use super::*;

#[test]
fn import_accepts_legacy_aliases_but_rejects_conflicting_or_unbounded_identity() {
    let request: ImportRequest =
        decode(json!({"provider":"codex","sessionId":"native","cwd":"/tmp"})).unwrap();
    assert_eq!(import_handle(&request).unwrap().session_id, "native");
    for extra in [
        json!({"providerId":"other"}),
        json!({"providerHandleId":"other"}),
        json!({"provider":""}),
        json!({"sessionId":"bad\nidentity"}),
        json!({"sessionId":"x".repeat(513)}),
        json!({"labels":{"key":"x".repeat(4097)}}),
    ] {
        let mut value = json!({"provider":"codex","sessionId":"native","cwd":"/tmp"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(import_handle(&decode(value).unwrap()).is_err());
    }
    assert!(decode::<ImportRequest>(json!({"provider":"codex","sessionId":"native"})).is_err());
    assert!(
        decode::<ImportRequest>(
            json!({"provider":"codex","sessionId":"native","cwd":"/tmp","unknown":true})
        )
        .is_err()
    );
}
