//! AIT control-plane daemon entry point.

use std::{future::IntoFuture, net::SocketAddr, path::PathBuf, sync::Arc};

use ait_agent_adapters::codex::{
    CodexAppServerAdapter, CodexAppServerConfig, CodexSessionTitleGenerator, CodexWorkspaceAgent,
};
use ait_application::{LocalControlService, PermissionPolicyLimits};
use ait_domain::SandboxAccess;
use ait_ports::{HostProviderModelCatalog, SessionTitleGenerator, WorkspaceAgent};
use ait_storage_sqlite::SqliteControlStore;
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, ValueEnum)]
enum MaximumSandbox {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl From<MaximumSandbox> for SandboxAccess {
    fn from(value: MaximumSandbox) -> Self {
        match value {
            MaximumSandbox::ReadOnly => Self::ReadOnly,
            MaximumSandbox::WorkspaceWrite => Self::WorkspaceWrite,
            MaximumSandbox::FullAccess => Self::FullAccess,
        }
    }
}

#[derive(Parser)]
struct Arguments {
    /// `SQLite` control-plane database.
    #[arg(long, default_value = "ait.sqlite3")]
    database: PathBuf,
    /// Loopback address exposed to local clients.
    #[arg(long, default_value = "127.0.0.1:7314")]
    listen: SocketAddr,
    /// Maximum Codex filesystem sandbox access the administrator permits.
    #[arg(long, value_enum, default_value = "full-access")]
    max_sandbox: MaximumSandbox,
    /// Disable session-scoped native approval grants.
    #[arg(long)]
    deny_session_approvals: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    if !arguments.listen.ip().is_loopback() {
        return Err("local API must bind a loopback address".into());
    }
    let store = Arc::new(SqliteControlStore::open(arguments.database)?);
    let adapter = Arc::new(CodexAppServerAdapter::new(CodexAppServerConfig::default())?);
    let codex: Arc<dyn WorkspaceAgent> = Arc::new(CodexWorkspaceAgent::new(adapter.clone()));
    let catalog: Arc<dyn HostProviderModelCatalog> = adapter.clone();
    let titles: Arc<dyn SessionTitleGenerator> = Arc::new(CodexSessionTitleGenerator::new(adapter));
    let service = Arc::new(
        LocalControlService::with_workspace_agent(store, codex)
            .with_permission_limits(PermissionPolicyLimits {
                max_sandbox: arguments.max_sandbox.into(),
                allow_session_approvals: !arguments.deny_session_approvals,
            })
            .with_provider_gateway(Arc::new(ait_agent_adapters::RigProviderGateway))
            .with_host_provider_catalog(catalog)
            .with_session_title_generator(titles),
    );
    let listener = tokio::net::TcpListener::bind(arguments.listen).await?;
    eprintln!("AIT daemon listening on http://{}", listener.local_addr()?);
    // Binding is the daemon ownership boundary. Startup scanning is read-only,
    // and every Run claim happens later while its Project advisory lock is held.
    let recovery_plan = service
        .prepare_startup_recovery()
        .await
        .map_err(|failure| {
            std::io::Error::other(format!(
                "failed to scan interrupted Runs ({}): {}",
                failure.code, failure.message
            ))
        })?;
    let recovery_count = recovery_plan.len();
    let recovery_service = service.clone();
    let mut recovery =
        tokio::spawn(async move { recovery_service.run_startup_recovery(recovery_plan).await });
    let server = axum::serve(listener, ait_api_http::router(service)).into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => {
            recovery.abort();
            result?;
        }
        result = &mut recovery => {
            match result {
                Ok(Ok(recovered)) if !recovered.is_empty() => {
                    eprintln!("reconciled {} Run(s) after startup", recovered.len());
                }
                Ok(Ok(_)) if recovery_count > 0 => {
                    eprintln!("startup recovery plan contained no runnable work");
                }
                Ok(Ok(_)) => {}
                Ok(Err(failure)) => eprintln!(
                    "startup recovery supervisor stopped ({}): {}",
                    failure.code, failure.message
                ),
                Err(failure) => eprintln!("startup recovery supervisor failed: {failure}"),
            }
            server.await?;
        }
    }
    Ok(())
}
