//! Exact thresholds, diagnostic values, and bounded accounting during cancellation.
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

fn delta(text: &str) -> AgentEvent {
    AgentEvent::MessageDelta {
        item_id: "item".into(),
        delta: text.into(),
    }
}

#[test]
fn item_lifecycle_counts_once_and_reports_the_first_excess() {
    let mut meter = Meter::default();
    let item = json!({"id":"one"});
    assert_eq!(
        meter.check(
            &AgentEvent::ItemStarted { item: item.clone() },
            Some(limits())
        ),
        Ok(())
    );
    assert_eq!(
        meter.check(&AgentEvent::ItemCompleted { item }, Some(limits())),
        Ok(())
    );
    let failure = meter
        .check(
            &AgentEvent::ItemStarted {
                item: json!({"id":"two"}),
            },
            Some(limits()),
        )
        .unwrap_err();
    assert_eq!(
        failure,
        LimitExceeded {
            metric: LimitMetric::NativeItems,
            actual: 2,
            limit: 1
        }
    );
    // Later buffered output cannot overwrite the cause or grow the item set.
    assert_eq!(
        meter.check(&delta(&"x".repeat(65)), Some(limits())),
        Err(failure)
    );
    assert_eq!(
        meter.check(
            &AgentEvent::ItemStarted {
                item: json!({"id":"three"})
            },
            Some(limits()),
        ),
        Err(failure)
    );
    assert_eq!(meter.items.len(), 2);
}

#[test]
fn individual_item_size_includes_json_escaping_and_accepts_the_exact_limit() {
    let item = json!({"text": "line\n\"quoted\"\\好", "data": [true, null, 42, 1.5]});
    let size = serde_json::to_vec(&item).unwrap().len();
    assert_eq!(serialized_size(&item), size as u64);
    let event = AgentEvent::ItemCompleted { item };
    assert_eq!(
        Meter::default().check(
            &event,
            Some(CodexExecutionLimits {
                max_output_bytes: size,
                ..limits()
            })
        ),
        Ok(())
    );
    assert_eq!(
        Meter::default().check(
            &event,
            Some(CodexExecutionLimits {
                max_output_bytes: size - 1,
                ..limits()
            })
        ),
        Err(LimitExceeded {
            metric: LimitMetric::ItemBytes,
            actual: size as u64,
            limit: (size - 1) as u64,
        })
    );
    let mut counter = ByteCounter::default();
    io::Write::flush(&mut counter).unwrap();
    assert_eq!(counter.0, 0);
}

#[test]
fn streamed_utf8_bytes_accumulate_across_items_without_counting_completed_text_twice() {
    let mut meter = Meter::default();
    assert_eq!(meter.check(&delta(&"x".repeat(61)), Some(limits())), Ok(()));
    assert_eq!(
        meter.check(
            &AgentEvent::ItemCompleted {
                item: json!({"id":"item", "text":"done"}),
            },
            Some(limits())
        ),
        Ok(())
    );
    assert_eq!(
        meter.check(
            &AgentEvent::MessageDelta {
                item_id: "another".into(),
                delta: "好".into(),
            },
            Some(limits())
        ),
        Ok(())
    );
    assert_eq!(
        meter.check(&delta("!"), Some(limits())),
        Err(LimitExceeded {
            metric: LimitMetric::TextBytes,
            actual: 65,
            limit: 64,
        })
    );
}

#[test]
fn reported_usage_counts_cached_input_once_and_checks_each_invocation() {
    let mut meter = Meter::default();
    let usage = AgentUsage {
        input_tokens: 8,
        cached_input_tokens: 7,
        output_tokens: 2,
        reasoning_output_tokens: 1,
        total_tokens: 10,
    };
    for _ in 0..2 {
        assert_eq!(
            meter.check(
                &AgentEvent::Usage {
                    usage: usage.clone()
                },
                Some(limits())
            ),
            Ok(())
        );
    }
    for (total_tokens, output_tokens) in [(11, 2), (0, 3)] {
        assert_eq!(
            Meter::default().check(
                &AgentEvent::Usage {
                    usage: AgentUsage {
                        output_tokens,
                        total_tokens,
                        ..usage.clone()
                    },
                },
                Some(limits())
            ),
            Err(LimitExceeded {
                metric: LimitMetric::Tokens,
                actual: 11,
                limit: 10,
            })
        );
    }
}

#[test]
fn counters_saturate_and_unmetered_events_do_not_consume_budget() {
    let mut meter = Meter {
        text_bytes: u64::MAX - 1,
        ..Meter::default()
    };
    assert_eq!(
        meter.check(&delta("two"), Some(limits())),
        Err(LimitExceeded {
            metric: LimitMetric::TextBytes,
            actual: u64::MAX,
            limit: 64,
        })
    );
    assert_eq!(
        Meter::default().check(
            &AgentEvent::Usage {
                usage: AgentUsage {
                    input_tokens: u64::MAX,
                    output_tokens: 1,
                    cached_input_tokens: 0,
                    reasoning_output_tokens: 0,
                    total_tokens: 0,
                },
            },
            Some(limits())
        ),
        Err(LimitExceeded {
            metric: LimitMetric::Tokens,
            actual: u64::MAX,
            limit: 10,
        })
    );
    let mut meter = Meter::default();
    assert_eq!(meter.check(&delta(&"x".repeat(65)), None), Ok(()));
    assert_eq!(meter.text_bytes, 0);
    assert_eq!(
        meter.check(
            &AgentEvent::ThreadStarted {
                thread_id: "thread".into()
            },
            Some(limits())
        ),
        Ok(())
    );
}

#[test]
fn domain_errors_include_readable_units_and_structured_measurements() {
    for (metric, name, label) in [
        (
            LimitMetric::NativeItems,
            "native_items",
            "native item count",
        ),
        (
            LimitMetric::ItemBytes,
            "item_bytes",
            "individual item size (bytes)",
        ),
        (
            LimitMetric::TextBytes,
            "text_bytes",
            "streamed text output (bytes)",
        ),
        (
            LimitMetric::Tokens,
            "tokens",
            "model invocation usage (tokens)",
        ),
    ] {
        let error = DomainError::from(LimitExceeded {
            metric,
            actual: 65,
            limit: 64,
        });
        assert_eq!(error.code, ErrorCode::RunLimitExceeded);
        assert_eq!(
            error.message,
            format!("Codex {label} limit exceeded: observed 65, limit 64")
        );
        assert_eq!(
            serde_json::to_value(error.details).unwrap(),
            json!({"metric":name, "actual":65, "limit":64})
        );
        assert!(!error.retryable);
        assert!(error.cause_id.is_none());
    }
}
