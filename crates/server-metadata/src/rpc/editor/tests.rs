use super::*;
use crate::protocol::editor::CAPABILITIES;

#[test]
fn legacy_editors_return_desktop_migration_without_opening_paths() {
    assert_eq!(
        execute(CAPABILITIES[0], json!({})).unwrap(),
        json!({"editors":[],"error":MOVED})
    );
    assert_eq!(
        execute(
            CAPABILITIES[1],
            json!({"path":"/missing","editorId":"code","mode":"reveal"})
        )
        .unwrap(),
        json!({"error":MOVED})
    );
    for params in [
        json!({}),
        json!({"path":"/x","editorId":" "}),
        json!({"path":"/x","editorId":"code","mode":"execute"}),
    ] {
        assert_eq!(
            execute(CAPABILITIES[1], params),
            Err(ErrorCode::InvalidMessage)
        );
    }
    assert_eq!(
        execute(CAPABILITIES[0], Value::Null),
        Err(ErrorCode::InvalidMessage)
    );
    assert_eq!(
        execute("unknown", json!({})),
        Err(ErrorCode::MethodNotFound)
    );
}
