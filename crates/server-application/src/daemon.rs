//! Daemon status, configuration, diagnostics, and installation-boundary use cases.

use std::fmt::Write as _;

use serde_json::Value;
use server_ports::daemon::{DaemonConfigReload, DaemonConfigStore, DaemonConfigStoreError};

/// Immutable runtime facts owned by one server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRuntime {
    /// Stable server identity.
    pub server_id: String,
    /// Running package version.
    pub version: Option<String>,
    /// Operating-system process ID.
    pub pid: u32,
    /// Current executable path.
    pub executable: String,
    /// RFC 3339 process start time.
    pub started_at: Option<String>,
    /// Actual bound listen address.
    pub listen: String,
}

/// Safe daemon application failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DaemonError {
    /// Persisted or proposed configuration is invalid.
    #[error("invalid daemon configuration")]
    InvalidConfig,
    /// Configuration persistence failed.
    #[error("daemon configuration I/O failed")]
    ConfigIo,
}

/// Application coordinator for daemon-level WebSocket operations.
#[derive(Debug)]
pub struct Daemon {
    runtime: DaemonRuntime,
    config: Box<dyn DaemonConfigStore>,
}

impl Daemon {
    /// Construct a daemon coordinator from immutable runtime facts and a new storage port.
    #[must_use]
    pub fn new(runtime: DaemonRuntime, config: Box<dyn DaemonConfigStore>) -> Self {
        Self { runtime, config }
    }

    /// Return immutable process status.
    #[must_use]
    pub fn runtime(&self) -> &DaemonRuntime {
        &self.runtime
    }

    /// Return current normalized mutable configuration.
    ///
    /// # Errors
    /// Returns an error when the persisted configuration is invalid or unavailable.
    pub fn get_config(&self) -> Result<Value, DaemonError> {
        self.config.get().map_err(config_error)
    }

    /// Persist and publish one normalized mutable configuration patch.
    ///
    /// # Errors
    /// Returns an error when the persisted configuration is invalid or unavailable.
    pub fn set_config(&self, patch: &Value) -> Result<Value, DaemonError> {
        self.config.patch(patch).map_err(config_error)
    }

    /// Reread externally edited configuration and classify changes.
    ///
    /// # Errors
    /// Returns an error when the persisted configuration is invalid or unavailable.
    pub fn reload_config(&self) -> Result<DaemonConfigReload, DaemonError> {
        self.config.reload().map_err(config_error)
    }

    /// Build a credential-free diagnostic report from process facts and installed surfaces.
    #[must_use]
    pub fn diagnostics(&self, capabilities: &[String], lifecycle: &str) -> String {
        let mut report = String::from("Paseo diagnostics\n\nDaemon\n");
        line(&mut report, "Server ID", &self.runtime.server_id);
        line(
            &mut report,
            "Version",
            self.runtime.version.as_deref().unwrap_or("unknown"),
        );
        line(&mut report, "PID", &self.runtime.pid.to_string());
        line(&mut report, "Executable", &self.runtime.executable);
        line(
            &mut report,
            "Started at",
            self.runtime.started_at.as_deref().unwrap_or("unknown"),
        );
        line(&mut report, "Listen", &self.runtime.listen);
        line(&mut report, "Lifecycle", lifecycle);
        report.push_str("\nSystem\n");
        line(&mut report, "OS", std::env::consts::OS);
        line(&mut report, "Architecture", std::env::consts::ARCH);
        line(
            &mut report,
            "CPU cores",
            &std::thread::available_parallelism()
                .map_or_else(|_| "unknown".to_owned(), |count| count.get().to_string()),
        );
        report.push_str("\nCapabilities\n");
        line(&mut report, "Count", &capabilities.len().to_string());
        line(&mut report, "Methods", &capabilities.join(", "));
        report.push_str("\nProviders\n  Total: 0\n  Available: 0\n  Unavailable: none\n");
        report
    }

    /// Return Paseo's update result shape for this installation.
    #[must_use]
    pub fn update_result(&self) -> DaemonUpdate {
        DaemonUpdate {
            success: false,
            error: Some(
                "Self-update is unavailable for this standalone Rust server installation"
                    .to_owned(),
            ),
            previous_version: self.runtime.version.clone(),
            new_version: None,
        }
    }
}

/// Installation-specific self-update result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonUpdate {
    /// Whether installation succeeded.
    pub success: bool,
    /// Safe error when it did not.
    pub error: Option<String>,
    /// Version before the attempt.
    pub previous_version: Option<String>,
    /// New version after a successful attempt.
    pub new_version: Option<String>,
}

fn line(report: &mut String, label: &str, value: &str) {
    let _ = writeln!(report, "  {label}: {value}");
}

fn config_error(error: DaemonConfigStoreError) -> DaemonError {
    match error {
        DaemonConfigStoreError::Invalid => DaemonError::InvalidConfig,
        DaemonConfigStoreError::Io => DaemonError::ConfigIo,
    }
}

#[cfg(test)]
mod tests;
