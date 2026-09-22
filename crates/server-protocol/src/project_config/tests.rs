use serde_json::json;

use super::*;

#[test]
fn raw_config_matches_paseo_known_field_validation_and_passthrough() {
    let parsed = PaseoConfigRaw::new(json!({
        "worktree": {
            "setup": ["npm ci"],
            "servicePorts": {"range":" 3000-4000 ", "portScript":" npm run port "},
            "future": true
        },
        "scripts":{"web":{"command":"npm run dev","future":1}},
        "metadataGeneration":{"title":{"instructions":42},"future":true},
        "future":{"nested":true}
    }))
    .unwrap();
    assert_eq!(
        parsed.value()["worktree"]["servicePorts"]["range"],
        "3000-4000"
    );
    assert_eq!(parsed.value()["metadataGeneration"]["title"], json!({}));
    assert_eq!(parsed.value()["future"], json!({"nested":true}));

    for invalid in [
        json!(null),
        json!({"worktree":{"setup":["ok", 2]}}),
        json!({"worktree":{"servicePorts":{}}}),
        json!({"worktree":{"servicePorts":{"range":"4000-3000"}}}),
        json!({"worktree":{"servicePorts":{"range":"3000-4000","extra":true}}}),
        json!({"scripts":{"web":"npm run dev"}}),
    ] {
        assert!(PaseoConfigRaw::new(invalid).is_err());
    }
}

#[test]
fn canonical_requests_use_camel_case_and_require_expected_revision() {
    let request: ProjectConfigWriteRequest = serde_json::from_value(json!({
        "repoRoot":"/repo",
        "config":{"worktree":{"setup":"npm ci"}},
        "expectedRevision":{"mtimeMs":12.5,"size":42}
    }))
    .unwrap();
    assert_eq!(request.repo_root, "/repo");
    assert!((request.expected_revision.unwrap().size - 42.0).abs() < f64::EPSILON);
    assert!(
        serde_json::from_value::<ProjectConfigWriteRequest>(json!({
            "repoRoot":"/repo", "config":{}
        }))
        .is_err()
    );
}

#[test]
fn result_boolean_discriminator_matches_paseo_payload() {
    assert_eq!(
        serde_json::to_value(ProjectConfigReadResult::Success {
            repo_root: "/repo".to_owned(),
            config: None,
            revision: None,
        })
        .unwrap(),
        json!({"ok":true,"repoRoot":"/repo","config":null,"revision":null})
    );
    assert_eq!(
        serde_json::to_value(ProjectConfigWriteResult::Failure {
            repo_root: "/repo".to_owned(),
            error: ProjectConfigRpcError::StaleProjectConfig {
                current_revision: None
            },
        })
        .unwrap(),
        json!({
            "ok":false,
            "repoRoot":"/repo",
            "error":{"code":"stale_project_config","currentRevision":null}
        })
    );
}
