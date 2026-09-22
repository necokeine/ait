use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::Parser;
use secrecy::SecretString;
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(
    name = "server",
    version,
    about = "Independent local server (M0 transport)"
)]
pub(super) struct Cli {
    /// Isolated server directory (default: `AIT_SERVER_DATA_DIR` or ~/.ait-server).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Loopback socket address (default: `AIT_SERVER_LISTEN`, config, or 127.0.0.1:7316).
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// Non-secret TOML configuration (default: <data-dir>/config.toml, if present).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Logging threshold: error, warn, info, debug, trace, or off.
    #[arg(long)]
    log_level: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    listen: Option<SocketAddr>,
    log_level: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct Config {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub token: SecretString,
    pub log_level: tracing::level_filters::LevelFilter,
}

impl Config {
    pub fn load(cli: Cli, env: impl Fn(&str) -> Option<OsString>) -> anyhow::Result<Self> {
        // Validate credentials before touching disk or opening the listener.
        let token = env("AIT_SERVER_TOKEN")
            .context("set AIT_SERVER_TOKEN before starting server")?
            .into_string()
            .map_err(|_| anyhow::anyhow!("AIT_SERVER_TOKEN must be UTF-8"))?;
        server_api::validate_token(&token)?;
        let token = SecretString::from(token);
        let data_dir = cli
            .data_dir
            .or_else(|| env("AIT_SERVER_DATA_DIR").map(PathBuf::from))
            .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".ait-server")))
            .context("set --data-dir or AIT_SERVER_DATA_DIR when HOME is unavailable")?;
        if data_dir.as_os_str().is_empty() {
            bail!("data directory must not be empty");
        }
        let explicit_config = cli.config.is_some();
        let config_path = cli.config.unwrap_or_else(|| data_dir.join("config.toml"));
        let file = match std::fs::read_to_string(&config_path) {
            Ok(text) => {
                // TOML diagnostics echo source lines, which may contain misplaced credentials.
                toml::from_str::<FileConfig>(&text).map_err(|_| {
                    anyhow::anyhow!(
                        "invalid server config; only listen and log_level are supported"
                    )
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit_config => {
                FileConfig::default()
            }
            Err(error) => return Err(error).context("read server config"),
        };
        let listen = if let Some(value) = cli.listen {
            value
        } else if let Some(value) = env("AIT_SERVER_LISTEN") {
            value
                .to_str()
                .context("AIT_SERVER_LISTEN must be UTF-8")?
                .parse()
                .context("parse AIT_SERVER_LISTEN")?
        } else {
            file.listen
                .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 7316)))
        };
        if !listen.ip().is_loopback() {
            bail!("M0 only supports loopback listening addresses");
        }
        let log_level = cli
            .log_level
            .or_else(|| env("AIT_SERVER_LOG_LEVEL").map(|v| v.to_string_lossy().into_owned()))
            .or(file.log_level)
            .unwrap_or_else(|| "info".to_owned())
            .parse()
            .context("parse log level")?;
        Ok(Self {
            data_dir,
            listen,
            token,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests;
