use super::*;
use serde_json::json;

#[test]
fn arbitrary_client_ids_keep_native_uuid_and_survive_replay() {
    let mut inputs = Inputs::default();
    let id = inputs.admit(Some("desktop-message-1")).unwrap();
    assert!(Uuid::parse_str(&id).is_ok());
    let restored = Inputs::restore(inputs.saved().as_ref()).unwrap();
    let mut item = super::super::streaming::entry(
        &id,
        json!({"type":"user_message","messageId":id,"text":"Hello"}),
        &Value::Null,
    );
    restored.decorate(&mut item);
    assert_eq!(item.item["clientMessageId"], "desktop-message-1");
    assert_eq!(inputs.admit(Some(&id)).unwrap(), id);
    assert!(Inputs::restore(Some(&json!({"invalid":"id"}))).is_err());
}
