use super::*;

#[test]
fn durable_plan_approval_preserves_identity_and_rejects_unrelated_authority() {
    let plan = Plan::receive(
        &json!({"id":"plan-1","text":"1. Fix the bug\n2. Run tests"}),
        "turn-1",
    )
    .unwrap()
    .unwrap();
    let restored = Plan::restore(Some(&json!(plan))).unwrap().unwrap();
    let prompt = plan.prepare(&json!({"behavior":"allow"})).unwrap().unwrap();
    assert_eq!(
        prompt.client_message_id,
        restored
            .prepare(&json!({"behavior":"allow"}))
            .unwrap()
            .unwrap()
            .client_message_id
    );
    assert!(prompt.text.contains("1. Fix the bug\n2. Run tests"));
    assert_eq!(plan.request()["actions"][1]["intent"], "implement");
    assert!(plan.prepare(&json!({"behavior":"deny"})).unwrap().is_none());
    assert!(
        plan.prepare(&json!({"behavior":"allow","scope":"session"}))
            .is_err()
    );
    assert!(
        plan.prepare(&json!({"behavior":"allow","updatedInput":{"plan":"different"}}))
            .is_err()
    );
    assert_eq!(
        plan.resolution(&json!({"behavior":"deny"})).unwrap().item["metadata"]["resolution"],
        "dismissed"
    );
    assert!(Plan::receive(&json!({"id":"plan","text":"x".repeat(48*1024+1)}), "turn").is_err());
    assert!(
        Plan::receive(&json!({"id":"plan","text":" "}), "turn")
            .unwrap()
            .is_none()
    );
}
