use super::*;

#[test]
fn error_code_has_stable_wire_value() {
    let encoded = serde_json::to_string(&ErrorCode::SessionPointerConflict).unwrap();
    assert_eq!(encoded, "\"SESSION_POINTER_CONFLICT\"");
    assert_eq!(
        serde_json::from_str::<ErrorCode>(&encoded).unwrap(),
        ErrorCode::SessionPointerConflict
    );
}
