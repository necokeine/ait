use serde_json::json;

use super::*;

#[test]
fn ping_echoes_valid_nonces_and_rejects_invalid_values() {
    assert_eq!(
        ping(json!({"nonce":"request-1","future":true})),
        Ok(json!({"nonce":"request-1"}))
    );
    assert_eq!(
        ping(json!({"nonce":"x".repeat(128)})),
        Ok(json!({"nonce":"x".repeat(128)}))
    );
    for value in [
        json!({}),
        json!(null),
        json!(["request-1"]),
        json!({"nonce":1}),
        json!({"nonce":""}),
        json!({"nonce":"x".repeat(129)}),
        json!({"nonce":"bad\nnonce"}),
    ] {
        assert_eq!(ping(value), Err(ErrorCode::InvalidMessage));
    }
}
