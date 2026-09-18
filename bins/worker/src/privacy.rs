//! Worker-local adapters for the shared fail-closed sensitive argument policy.
use serde_json::Value;

pub(crate) fn redact_display(value: &mut Value) {
    ait_contracts::sensitive::redact_sensitive_display(value);
}

#[cfg(test)]
mod tests;
