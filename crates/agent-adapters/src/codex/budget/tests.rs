//! Limits count distinct items and actual bytes without double-counting cached tokens.
use super::*;
use crate::AgentUsage;
use serde_json::json;

fn limits() -> CodexExecutionLimits {
    CodexExecutionLimits {
        max_steps: 1,
        max_tokens: 10,
        max_output_bytes: 64,
    }
}
#[test]
fn item_lifecycle_counts_once_and_oversized_items_fail_closed() {
    let mut meter = Meter::default();
    let item = json!({"id":"one"});
    assert!(!meter.exceeded(
        &AgentEvent::ItemStarted { item: item.clone() },
        Some(limits())
    ));
    assert!(!meter.exceeded(&AgentEvent::ItemCompleted { item }, Some(limits())));
    assert!(meter.exceeded(
        &AgentEvent::ItemStarted {
            item: json!({"id":"two"})
        },
        Some(limits())
    ));
    assert!(Meter::default().exceeded(
        &AgentEvent::ItemStarted {
            item: json!({"text":"x".repeat(64)})
        },
        Some(limits())
    ));
}
#[test]
fn streamed_utf8_bytes_are_accumulated_within_one_turn() {
    let mut meter = Meter::default();
    let delta = |text: String| AgentEvent::MessageDelta {
        item_id: "item".into(),
        delta: text,
    };
    assert!(!meter.exceeded(&delta("x".repeat(63)), Some(limits())));
    assert!(meter.exceeded(&delta("好".into()), Some(limits())));
    assert!(!meter.exceeded(&delta("unlimited".into()), None));
}
#[test]
fn reported_usage_counts_cached_input_once_and_rejects_excess() {
    let mut meter = Meter::default();
    let mut usage = AgentUsage {
        input_tokens: 8,
        cached_input_tokens: 7,
        output_tokens: 2,
        reasoning_output_tokens: 1,
        total_tokens: 10,
    };
    assert!(!meter.exceeded(
        &AgentEvent::Usage {
            usage: usage.clone()
        },
        Some(limits())
    ));
    usage.total_tokens = 11;
    assert!(meter.exceeded(
        &AgentEvent::Usage {
            usage: usage.clone()
        },
        Some(limits())
    ));
    usage.total_tokens = 0;
    usage.output_tokens = 3;
    assert!(meter.exceeded(&AgentEvent::Usage { usage }, Some(limits())));
}
