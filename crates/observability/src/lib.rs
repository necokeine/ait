//! Structured, correlated, and redacted local observability primitives.

use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Replacement used for sensitive structured fields.
pub const REDACTED: &str = "[REDACTED]";

/// Stable correlation labels shared by logs and metrics.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Correlation {
    /// Project boundary, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Interactive Session, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Run identity, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Request or `ToolUse` call identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

/// Severity of a structured event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Diagnostic detail.
    Debug,
    /// Normal lifecycle event.
    Info,
    /// Recoverable or degraded behavior.
    Warn,
    /// Failed operation.
    Error,
}

/// One newline-delimited JSON log record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: i64,
    /// Event severity.
    pub level: Level,
    /// Stable subsystem or module name.
    pub target: String,
    /// Stable, non-sensitive event name.
    pub event: String,
    /// Cross-layer correlation identifiers.
    #[serde(flatten)]
    pub correlation: Correlation,
    /// Additional structured data, recursively redacted before output.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
}

/// Destination for structured records.
pub trait LogSink: Send + Sync {
    /// Emits one record after applying default redaction.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the destination cannot accept the record.
    fn emit(&self, record: &LogRecord) -> io::Result<()>;
}

/// Thread-safe newline-delimited JSON sink.
pub struct JsonLogSink<W> {
    writer: Mutex<W>,
}

impl<W> JsonLogSink<W> {
    /// Wraps a writable destination.
    #[must_use]
    pub const fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
        }
    }
}

impl<W: Write + Send> LogSink for JsonLogSink<W> {
    fn emit(&self, record: &LogRecord) -> io::Result<()> {
        let mut safe = record.clone();
        redact_fields(&mut safe.fields);
        let encoded = serde_json::to_vec(&safe).map_err(io::Error::other)?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("log sink lock poisoned"))?;
        writer.write_all(&encoded)?;
        writer.write_all(b"\n")
    }
}

/// One correlated counter sample.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MetricPoint {
    /// Stable metric name.
    pub name: String,
    /// Cross-layer correlation identifiers.
    #[serde(flatten)]
    pub correlation: Correlation,
    /// Monotonic counter value.
    pub value: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MetricKey {
    name: String,
    correlation: Correlation,
}

/// In-process monotonic metrics registry.
#[derive(Clone, Default)]
pub struct MetricsRegistry {
    counters: Arc<Mutex<BTreeMap<MetricKey, u64>>>,
}

impl MetricsRegistry {
    /// Adds `amount` to a correlated counter, saturating on overflow.
    pub fn increment(&self, name: impl Into<String>, correlation: Correlation, amount: u64) {
        let key = MetricKey {
            name: name.into(),
            correlation,
        };
        if let Ok(mut counters) = self.counters.lock() {
            let value = counters.entry(key).or_default();
            *value = value.saturating_add(amount);
        }
    }

    /// Returns a deterministic snapshot suitable for a local metrics endpoint.
    #[must_use]
    pub fn snapshot(&self) -> Vec<MetricPoint> {
        self.counters.lock().map_or_else(
            |_| Vec::new(),
            |counters| {
                counters
                    .iter()
                    .map(|(key, value)| MetricPoint {
                        name: key.name.clone(),
                        correlation: key.correlation.clone(),
                        value: *value,
                    })
                    .collect()
            },
        )
    }
}

/// Combined logging and metrics handle shared by transport adapters.
#[derive(Clone)]
pub struct Telemetry {
    logs: Arc<dyn LogSink>,
    metrics: MetricsRegistry,
}

impl Telemetry {
    /// Creates a telemetry handle with an injectable log destination.
    #[must_use]
    pub fn new(logs: Arc<dyn LogSink>) -> Self {
        Self {
            logs,
            metrics: MetricsRegistry::default(),
        }
    }

    /// Creates the production default writing redacted JSON lines to stderr.
    #[must_use]
    pub fn stderr() -> Self {
        Self::new(Arc::new(JsonLogSink::new(io::stderr())))
    }

    /// Emits a record. Logging failures are intentionally non-fatal.
    pub fn emit(&self, record: &LogRecord) {
        let _ = self.logs.emit(record);
    }

    /// Returns the shared registry.
    #[must_use]
    pub const fn metrics(&self) -> &MetricsRegistry {
        &self.metrics
    }
}

/// Recursively redacts sensitive keys and bearer values in structured data.
pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if is_sensitive_key(key) {
                    *value = Value::String(REDACTED.into());
                } else {
                    redact_value(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_value),
        Value::String(text) if looks_like_bearer(text) => *text = REDACTED.into(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn redact_fields(fields: &mut BTreeMap<String, Value>) {
    for (key, value) in fields {
        if is_sensitive_key(key) {
            *value = Value::String(REDACTED.into());
        } else {
            redact_value(value);
        }
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '.'], "_");
    [
        "authorization",
        "cookie",
        "password",
        "secret",
        "token",
        "api_key",
        "credential",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn looks_like_bearer(value: &str) -> bool {
    value
        .trim_start()
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn logs_are_correlated_and_redacted_by_default() {
        let writer = SharedWriter::default();
        let bytes = writer.0.clone();
        let telemetry = Telemetry::new(Arc::new(JsonLogSink::new(writer)));
        telemetry.emit(&LogRecord {
            timestamp_ms: 7,
            level: Level::Info,
            target: "api".into(),
            event: "command.completed".into(),
            correlation: Correlation {
                project_id: Some("p1".into()),
                session_id: Some("s1".into()),
                run_id: Some("r1".into()),
                call_id: Some("c1".into()),
            },
            fields: BTreeMap::from([
                ("authorization".into(), Value::String("Bearer abc".into())),
                ("nested".into(), serde_json::json!({"api-key": "abc"})),
            ]),
        });
        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(output.contains("\"project_id\":\"p1\""));
        assert!(output.contains("\"session_id\":\"s1\""));
        assert!(output.contains("\"run_id\":\"r1\""));
        assert!(output.contains("\"call_id\":\"c1\""));
        assert!(!output.contains("Bearer abc"));
        assert!(!output.contains("\"abc\""));
    }

    #[test]
    fn metrics_keep_the_same_correlation_labels() {
        let registry = MetricsRegistry::default();
        let correlation = Correlation {
            project_id: Some("p1".into()),
            session_id: Some("s1".into()),
            run_id: Some("r1".into()),
            call_id: Some("c1".into()),
        };
        registry.increment("commands_total", correlation.clone(), 1);
        registry.increment("commands_total", correlation.clone(), 2);
        assert_eq!(
            registry.snapshot(),
            vec![MetricPoint {
                name: "commands_total".into(),
                correlation,
                value: 3,
            }]
        );
    }
}
