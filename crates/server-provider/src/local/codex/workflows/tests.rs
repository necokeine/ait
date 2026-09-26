use super::*;
#[cfg(unix)]
use crate::ports::agent_session::{AgentClient, AgentTurnEvent};
#[cfg(unix)]
use crate::test_support::Fixture;

#[test]
fn version_gate_does_not_guess_capabilities_from_unknown_agents() {
    assert_eq!(
        version("codex_cli_rs/0.153.4 (Mac OS 26.0)"),
        Some((0, 153, 4))
    );
    assert_eq!(version("unknown"), None);
    assert_eq!(version("0.128"), None);
    assert_eq!(version("1.2.3.4"), None);
    let client = CodexClient::new("unused".into());
    let config = StoredAgentConfig {
        mode_id: Some("auto-review".into()),
        ..Default::default()
    };
    assert_eq!(
        client.validate_workflows(&config),
        Err(AgentSessionError::Rejected)
    );
    client.capabilities.store(REVIEW | PLAN, Ordering::Relaxed);
    assert!(client.validate_workflows(&config).is_ok());
    assert_eq!(reviewer(&config), "auto_review");
    assert!(
        client
            .modes()
            .iter()
            .any(|mode| mode["id"] == "auto-review")
    );
    assert!(
        client
            .features(&config)
            .iter()
            .any(|feature| feature["id"] == "plan_mode")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn plan_selection_uses_native_presets_and_can_return_to_default_workflow() {
    let fixture = Fixture::new();
    fixture.mode("workflows");
    let client = fixture.client();
    let mut spec = fixture.spec();
    let details = client.discover(&spec.cwd).await.unwrap();
    assert!(
        details
            .features
            .iter()
            .any(|feature| feature["id"] == "plan_mode")
    );
    assert!(details.modes.iter().any(|mode| mode["id"] == "auto-review"));
    spec.config =
        serde_json::from_value(json!({"modeId":"auto-review","featureValues":{"plan_mode":true}}))
            .unwrap();
    let mut session = client.create_session(&spec).await.unwrap();
    session.start_turn("plan", &spec.config).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if matches!(
                session.poll_turn().unwrap(),
                Some(AgentTurnEvent::Completed(_))
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    spec.config.feature_values = None;
    spec.config.mode_id = Some("auto".into());
    session.start_turn("implement", &spec.config).await.unwrap();
    session.close().await.unwrap();
    let requests = fixture.requests();
    let turns: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(turns[0]["params"]["collaborationMode"]["mode"], "plan");
    assert_eq!(
        turns[0]["params"]["collaborationMode"]["settings"]["model"],
        "offline-model"
    );
    assert_eq!(turns[0]["params"]["approvalsReviewer"], "auto_review");
    assert_eq!(turns[1]["params"]["collaborationMode"]["mode"], "default");
    assert_eq!(turns[1]["params"]["approvalsReviewer"], "user");
}
