//! AIT control-plane daemon entry point.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use ait_agent_adapters::codex::{
    CodexAppServerAdapter, CodexAppServerConfig, CodexSessionTitleGenerator, CodexWorkspaceAgent,
};
use ait_application::LocalControlService;
use ait_ports::{SessionTitleGenerator, WorkspaceAgent};
use ait_storage_sqlite::SqliteControlStore;
use clap::Parser;

#[derive(Parser)]
struct Arguments {
    /// `SQLite` control-plane database.
    #[arg(long, default_value = "ait.sqlite3")]
    database: PathBuf,
    /// Loopback address exposed to local clients.
    #[arg(long, default_value = "127.0.0.1:7314")]
    listen: SocketAddr,
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
    let titles: Arc<dyn SessionTitleGenerator> = Arc::new(CodexSessionTitleGenerator::new(adapter));
    let service = Arc::new(
        LocalControlService::with_workspace_agent(store, codex)
            .with_session_title_generator(titles),
    );
    let listener = tokio::net::TcpListener::bind(arguments.listen).await?;
    eprintln!("AIT daemon listening on http://{}", listener.local_addr()?);
    axum::serve(listener, ait_api_http::router(service)).await?;
    Ok(())
}
