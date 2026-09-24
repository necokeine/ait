use std::fs;

use crate::ports::daemon::DaemonConfigStore;
use serde_json::json;

use super::*;

fn store(directory: &tempfile::TempDir) -> FileDaemonConfigStore {
    FileDaemonConfigStore::new(
        directory.path().join("config.json"),
        json!({
            "relay":{"enabled":false},
            "mcp":{"enabled":true,"injectIntoAgents":false},
            "browserTools":{"enabled":false},
            "providers":{"codex":{"enabled":true}},
            "metadataGeneration":{"providers":[{"provider":"codex"}]},
            "autoArchiveAfterMerge":false,
            "enableTerminalAgentHooks":false,
            "appendSystemPrompt":""
        }),
    )
}

#[test]
fn initializes_and_atomically_patches_supported_fields() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    assert!(!directory.path().join("config.json").exists());
    assert_eq!(store.get().unwrap()["relay"]["enabled"], false);
    let next = store
        .patch(&json!({
            "relay":{"enabled":true},
            "providers":{"codex":{"enabled":false,"future":1}},
            "unknown":{"ignored":true}
        }))
        .unwrap();
    assert_eq!(next["relay"]["enabled"], true);
    assert_eq!(next["providers"]["codex"]["enabled"], false);
    assert_eq!(next["providers"]["codex"]["future"], 1);
    assert!(next.get("unknown").is_none());
    let persisted: Value =
        serde_json::from_slice(&fs::read(directory.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(persisted, next);
}

#[test]
fn provider_removal_updates_overrides_and_metadata_candidates() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let next = store.patch(&json!({"removeProviders":["codex"]})).unwrap();
    assert!(next["providers"].as_object().unwrap().is_empty());
    assert!(
        next["metadataGeneration"]["providers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reload_classifies_live_and_restart_paths() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    store.get().unwrap();
    fs::write(
        directory.path().join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "relay":{"enabled":true},
            "mcp":{"enabled":true,"injectIntoAgents":false},
            "browserTools":{"enabled":false},
            "providers":{"codex":{"enabled":true}},
            "metadataGeneration":{"providers":[{"provider":"codex"}]},
            "autoArchiveAfterMerge":false,
            "enableTerminalAgentHooks":false,
            "appendSystemPrompt":"",
            "plugins":{"local":{"source":"directory"}},
            "futureRoot":true
        }))
        .unwrap(),
    )
    .unwrap();
    let reloaded = store.reload().unwrap();
    assert_eq!(reloaded.applied_paths, ["daemon.relay.enabled"]);
    assert_eq!(reloaded.restart_required_paths, ["futureRoot", "plugins"]);
    assert!(reloaded.override_controlled_paths.is_empty());
}

#[test]
fn invalid_or_symlink_documents_are_rejected_without_replacing_current() {
    let directory = tempfile::tempdir().unwrap();
    let config_store = store(&directory);
    let current = config_store.get().unwrap();
    fs::write(directory.path().join("config.json"), b"not-json").unwrap();
    assert_eq!(config_store.reload(), Err(DaemonConfigStoreError::Invalid));
    assert_eq!(config_store.get().unwrap(), current);

    #[cfg(unix)]
    {
        let other = directory.path().join("other.json");
        fs::write(&other, b"{}").unwrap();
        fs::remove_file(directory.path().join("config.json")).unwrap();
        std::os::unix::fs::symlink(other, directory.path().join("config.json")).unwrap();
        let fresh = store(&directory);
        assert_eq!(fresh.get(), Err(DaemonConfigStoreError::Io));
    }
}
