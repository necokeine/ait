use super::*;
use serde_json::json;

#[test]
fn missing_usage_is_omitted_and_nonfinite_or_imprecise_facts_are_invalid() {
    assert_eq!(
        serde_json::to_value(AgentUsage::default()).unwrap(),
        json!({})
    );
    for cost in [-1.0, f64::INFINITY, f64::NAN] {
        assert!(
            !AgentUsage {
                total_cost_usd: Some(cost),
                ..Default::default()
            }
            .is_valid()
        );
    }
    assert!(
        !AgentUsage {
            input_tokens: Some(u64::MAX),
            ..Default::default()
        }
        .is_valid()
    );
    let usage: AgentUsage =
        serde_json::from_value(json!({"inputTokens":0,"totalCostUsd":0.1})).unwrap();
    assert!(usage.is_valid());
    assert_eq!(
        serde_json::to_value(usage).unwrap(),
        json!({"inputTokens":0,"totalCostUsd":0.1})
    );
}
