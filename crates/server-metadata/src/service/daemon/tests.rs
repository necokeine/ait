use std::sync::Mutex;

use crate::ports::daemon::{DaemonConfigReload, DaemonConfigStore, DaemonConfigStoreError};
use serde_json::json;

use super::*;

#[derive(Debug)]
struct Config {
    value: Mutex<Value>,
}

impl DaemonConfigStore for Config {
    fn get(&self) -> Result<Value, DaemonConfigStoreError> {
        Ok(self.value.lock().unwrap().clone())
    }

    fn patch(&self, patch: &Value) -> Result<Value, DaemonConfigStoreError> {
        let mut value = self.value.lock().unwrap();
        value["patch"] = patch.clone();
        Ok(value.clone())
    }

    fn reload(&self) -> Result<DaemonConfigReload, DaemonConfigStoreError> {
        Ok(DaemonConfigReload {
            config: self.get()?,
            applied_paths: vec!["daemon.relay.enabled".to_owned()],
            restart_required_paths: Vec::new(),
            override_controlled_paths: Vec::new(),
        })
    }
}

fn daemon() -> Daemon {
    Daemon::new(
        DaemonRuntime {
            server_id: "srv-1".to_owned(),
            version: Some("1.2.3".to_owned()),
            pid: 42,
            executable: "/bin/server".to_owned(),
            started_at: Some("2026-09-22T00:00:00Z".to_owned()),
            listen: "127.0.0.1:6767".to_owned(),
        },
        Box::new(Config {
            value: Mutex::new(json!({"relay":{"enabled":false}})),
        }),
    )
}

#[test]
fn coordinates_config_get_patch_and_reload() {
    let daemon = daemon();
    assert_eq!(daemon.get_config().unwrap()["relay"]["enabled"], false);
    assert_eq!(
        daemon
            .set_config(&json!({"relay":{"enabled":true}}))
            .unwrap()["patch"]["relay"]["enabled"],
        true
    );
    assert_eq!(
        daemon.reload_config().unwrap().applied_paths,
        ["daemon.relay.enabled"]
    );
}

#[test]
fn status_diagnostics_and_update_are_sanitized_and_stable() {
    let daemon = daemon();
    assert_eq!(daemon.runtime().server_id, "srv-1");
    let diagnostic = daemon.diagnostics(&["daemon.get_status.request".to_owned()], "ready");
    assert!(diagnostic.contains("Server ID: srv-1"));
    assert!(diagnostic.contains("Methods: daemon.get_status.request"));
    assert!(!diagnostic.contains("token"));
    assert_eq!(
        daemon.update_result(),
        DaemonUpdate {
            success: false,
            error: Some(
                "Self-update is unavailable for this standalone Rust server installation"
                    .to_owned()
            ),
            previous_version: Some("1.2.3".to_owned()),
            new_version: None,
        }
    );
}
