//! Independent local server entry point.

mod config;
mod host;
mod instance;

use std::process::ExitCode;

use anyhow::Context;
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = config::Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .try_init();
            tracing::error!(error = %format!("{error:#}"), "server stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: config::Cli) -> anyhow::Result<()> {
    let config = config::Config::load(cli, |name| std::env::var_os(name))
        .context("load server configuration")?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(config.log_level)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize logging: {error}"))?;
    let shutdown = shutdown_signal()?;
    let server = host::Server::bind(config).await?;
    tracing::info!(listen = %server.address(), "server ready");
    server.serve(shutdown).await
}

fn shutdown_signal() -> anyhow::Result<impl Future<Output = ()>> {
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("register SIGTERM handler")?;
    Ok(async move {
        #[cfg(unix)]
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result { tracing::error!(%error, "read interrupt signal"); }
            },
            _ = terminate.recv() => {},
        }
        #[cfg(not(unix))]
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "read interrupt signal");
        }
    })
}
