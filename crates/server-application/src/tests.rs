use super::*;

#[test]
fn idempotency_keys_are_bounded_ascii_without_whitespace() {
    for key in ["", "with space", "bad\nkey", "非ascii", &"a".repeat(129)] {
        assert_eq!(validate_key(key), Err(ProjectError::Invalid));
    }
    assert!(validate_key(&"a".repeat(128)).is_ok());
}
