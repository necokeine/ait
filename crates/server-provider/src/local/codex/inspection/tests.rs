use super::*;

#[test]
fn usage_preserves_buckets_reports_missing_windows_and_rejects_malformed_metrics() {
    let result = usage(&json!({"rateLimitsByLimitId":{"codex":{"planType":"plus","primary":{"usedPercent":105,"resetsAt":1_700_000_000}},"review":{"secondary":{"usedPercent":80}}}})).unwrap();
    assert_eq!(result["windows"][0]["remainingPct"], 0);
    assert_eq!(result["windows"][0]["tone"], "danger");
    assert_eq!(result["windows"][1]["tone"], "warning");
    assert_eq!(result["planLabel"], "plus");
    assert_eq!(
        usage(&json!({"rateLimits":{}})).unwrap()["status"],
        "unavailable"
    );
    for response in [
        json!({}),
        json!({"rateLimits":{"primary":{"usedPercent":-1}}}),
        json!({"rateLimits":{"primary":{"usedPercent":1,"resetsAt":i64::MAX}}}),
    ] {
        assert!(usage(&response).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn diagnostics_do_not_expose_private_account_fields() {
    let fixture = crate::test_support::Fixture::new();
    let script = std::fs::read_to_string(&fixture.program).unwrap().replace(
        "root = Path.cwd()",
        &format!(
            "root = Path({})",
            serde_json::to_string(&fixture.cwd).unwrap()
        ),
    );
    std::fs::write(&fixture.program, script).unwrap();
    let diagnostic = fixture.client().diagnostic().await.unwrap();
    assert!(diagnostic.contains("ChatGPT login"));
    assert!(!diagnostic.contains("private@example.test"));
    let result = fixture.client().usage().await.unwrap();
    assert_eq!(result["status"], "available");
    assert_eq!(result["windows"][0]["usedPct"], 25);
    let client = CodexClient::new(fixture.root.path().join("missing"));
    assert_eq!(
        client.diagnostic().await.unwrap(),
        "Codex executable: unavailable"
    );
    assert!(client.usage().await.is_err());
}
