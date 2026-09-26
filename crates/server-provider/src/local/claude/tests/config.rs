use super::*;
use crate::local::claude::config;

#[test]
fn rejects_unsupported_configuration_and_preserves_native_modes() {
    let client = ClaudeClient::new("missing-claude".into());
    assert_eq!(client.provider(), "claude");
    for mode in [
        "default",
        "plan",
        "acceptEdits",
        "auto",
        "bypassPermissions",
    ] {
        assert!(
            client
                .validate_config(&StoredAgentConfig {
                    mode_id: Some(mode.into()),
                    ..StoredAgentConfig::default()
                })
                .is_ok()
        );
    }
    for value in [
        json!({"model":""}),
        json!({"model":"bad\nmodel"}),
        json!({"thinkingOptionId":"bad"}),
        json!({"modeId":"full-access"}),
        json!({"featureValues":{"fast_mode":true}}),
        json!({"providerOptions":{"args":[]}}),
        json!({"toolPolicy":{}}),
        json!({"mcpServers":{"test":{}}}),
    ] {
        assert_eq!(
            client.validate_config(&serde_json::from_value(value).unwrap()),
            Err(AgentSessionError::Rejected)
        );
    }
    let settings = client.settings(&StoredAgentConfig::default());
    assert_eq!(settings["availableModes"].as_array().unwrap().len(), 5);
    assert_eq!(settings["capabilities"]["supportsStreaming"], true);
    assert_eq!(settings["capabilities"]["supportsSessionListing"], true);
    assert!(config::validate_spec(&spec(std::path::Path::new("relative"))).is_err());
}

#[test]
fn discovery_uses_cli_models_efforts_and_commands() {
    let response = json!({"models":[{"value":"default","displayName":"Default","supportedEffortLevels":["low","max"]},
        {"value":"custom","displayName":"Custom gateway"}],"commands":[{"name":"review","description":"Review"}]});
    let models = config::models(&response).unwrap();
    assert_eq!(models[0]["thinkingOptions"][1]["id"], "max");
    assert_eq!(models[0]["isDefault"], true);
    assert_eq!(models[1]["thinkingOptions"], json!([]));
    assert_eq!(config::commands(&response).unwrap()[0]["name"], "review");
    for bad in [
        json!({}),
        json!({"models":[]}),
        json!({"models":[{"value":"x"}]}),
        json!({"models":[{"value":"x","displayName":"X","supportedEffortLevels":[false]}]}),
    ] {
        assert!(config::models(&bad).is_err());
    }
    assert!(config::commands(&json!({"commands":[{}]})).is_err());
}

#[tokio::test]
async fn missing_executable_reports_safe_availability_and_diagnostic() {
    let client = ClaudeClient::new("/does-not-exist/claude".into());
    assert!(!client.is_available().await.unwrap());
    assert!(client.diagnostic().await.unwrap().contains("unavailable"));
    assert!(client.usage().await.is_err());
}

#[test]
fn first_party_fast_and_thinking_capabilities_do_not_leak_to_custom_aliases() {
    for model in [
        "claude-opus-5",
        "claude-opus-4-6[1m]",
        "Opus 4.7",
        "claude-opus-4-8-20260101",
    ] {
        let config = serde_json::from_value(
            json!({"model":model,"thinkingOptionId":"off","featureValues":{"fast_mode":true}}),
        )
        .unwrap();
        config::validate(&config).unwrap();
        assert_eq!(config::features(&config)[0]["value"], true);
    }
    for model in [
        "claude-sonnet-5",
        "custom-opus",
        "gateway/claude-opus-5",
        "opus",
    ] {
        let config =
            serde_json::from_value(json!({"model":model,"featureValues":{"fast_mode":true}}))
                .unwrap();
        assert_eq!(config::validate(&config), Err(AgentSessionError::Rejected));
        assert!(config::features(&config).is_empty());
    }
    let response = json!({"models":[{"value":"claude-opus-5","displayName":"Opus","supportedEffortLevels":["xhigh"]}]});
    let models = config::models(&response).unwrap();
    assert_eq!(
        models[0]["thinkingOptions"],
        json!([{"id":"off","label":"Off"},{"id":"xhigh","label":"xhigh"},{"id":"ultracode","label":"Ultra Code"}])
    );
}

#[test]
fn native_resolved_aliases_drive_discovery_validation_and_feature_controls() {
    let client = ClaudeClient::new("unused".into());
    let response = json!({"models":[{"value":"default","resolvedModel":"claude-opus-4-6","displayName":"Default","supportedEffortLevels":["high","max"]},
        {"value":"sonnet","resolvedModel":"claude-sonnet-4-6","displayName":"Sonnet","supportedEffortLevels":["high"]}]});
    client.remember_models(&response).unwrap();
    let config: StoredAgentConfig = serde_json::from_value(
        json!({"featureValues":{"fast_mode":true},"thinkingOptionId":"off"}),
    )
    .unwrap();
    client.validate_config(&config).unwrap();
    assert_eq!(client.settings(&config)["features"][0]["value"], true);
    let models = config::models(&response).unwrap();
    assert_eq!(models[0]["supportsFastMode"], true);
    assert_eq!(models[1]["supportsFastMode"], false);
    assert_eq!(models[1]["thinkingOptions"][0]["id"], "off");
    assert!(
        client
            .validate_config(&StoredAgentConfig {
                model: Some("sonnet".into()),
                ..config
            })
            .is_err()
    );
}
