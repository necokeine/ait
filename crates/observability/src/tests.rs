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
