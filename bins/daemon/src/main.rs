//! AIT control-plane daemon entry point.

use std::{future::IntoFuture, net::SocketAddr, path::PathBuf, sync::Arc};

use ait_agent_adapters::codex::{
    CodexAppServerAdapter, CodexAppServerConfig, CodexSessionTitleGenerator,
};
use ait_application::{LocalControlService, PermissionPolicyLimits};
use ait_domain::SandboxAccess;
use ait_ports::{HostProviderModelCatalog, SessionTitleGenerator, WorkspaceAgent};
use ait_storage_sqlite::SplitSqliteControlStore as SqliteControlStore;
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
    /// Global `SQLite` catalog; Project histories live in `<project>/.ait/project.sqlite3`.
    #[arg(long, default_value = "ait.sqlite3")]
    database: PathBuf,
    /// Loopback address exposed to local clients.
    #[arg(long, default_value = "127.0.0.1:7314")]
    listen: SocketAddr,
    /// Maximum filesystem sandbox access permitted for every provider.
    /// This ceiling does not change the `read_only` default for new Runs.
    #[arg(long, value_enum, default_value = "full-access")]
    max_sandbox: MaximumSandbox,
    /// Disable session-scoped native approval grants.
    #[arg(long)]
    deny_session_approvals: bool,
    /// Trusted worker executable; defaults to ait-worker beside ait-daemon.
    #[arg(long)]
    worker_binary: Option<PathBuf>,
    /// Strict per-Run cost ceiling in millionths of the billing currency.
    /// Providers without verifiable prices are denied before invocation.
    #[arg(long)]
    max_run_cost_micros: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    if !arguments.listen.ip().is_loopback() {
        return Err("local API must bind a loopback address".into());
    }
    let store = Arc::new(SqliteControlStore::open(arguments.database)?);
    let worker_binary = match arguments.worker_binary {
        Some(path) => path,
        None => std::env::current_exe()?.with_file_name(if cfg!(windows) {
            "ait-worker.exe"
        } else {
            "ait-worker"
        }),
    };
    let supervisor = Arc::new(
        ait_ipc::supervisor::WorkerSupervisor::new(worker_binary)
            .with_cost_ceiling(arguments.max_run_cost_micros),
    );
    let adapter = Arc::new(CodexAppServerAdapter::new(CodexAppServerConfig::default())?);
    let codex: Arc<dyn WorkspaceAgent> = supervisor.clone();
    let catalog: Arc<dyn HostProviderModelCatalog> = adapter.clone();
    let titles: Arc<dyn SessionTitleGenerator> = Arc::new(CodexSessionTitleGenerator::new(adapter));
    let mut service = LocalControlService::with_workspace_agent(store, codex)
        .with_project_directory_creator(Arc::new(
            ait_project_local::DocumentsProjectDirectory::default(),
        ))
        .with_permission_limits(PermissionPolicyLimits {
            max_sandbox: arguments.max_sandbox.into(),
            allow_session_approvals: !arguments.deny_session_approvals,
        })
        .with_provider_gateway(Arc::new(ait_agent_adapters::RigProviderGateway))
        .with_api_tools(Arc::new(ait_tools::host::HostToolFactory))
        .with_run_dispatcher(supervisor.clone())
        .with_host_provider_catalog(catalog);
    if arguments.max_run_cost_micros.is_none() {
        service = service.with_session_title_generator(titles);
    }
    let service = Arc::new(service);
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
    for (project_id, failure) in recovery_plan.unavailable_projects() {
        eprintln!("Project {project_id} startup recovery deferred: {failure}");
    }
    let recovery_count = recovery_plan.len();
    let recovery_service = service.clone();
    let mut recovery =
        tokio::spawn(async move { recovery_service.run_startup_recovery(recovery_plan).await });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, ait_api_http::router(service.clone()))
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .into_future();
    tokio::pin!(server);
    let mut recovery_done = false;
    loop {
        tokio::select! {
            result=&mut server=>{result?;break;},
            ()=shutdown_signal()=>break,
            result=&mut recovery,if !recovery_done=>{
                recovery_done=true;
                match result {
                    Ok(Ok(recovered)) if recovery_count>0=>eprintln!("reconciled {} Run(s) after startup",recovered.len()),
                    Ok(Ok(_))=>{},
                    _=>eprintln!("startup recovery stopped; inspect durable Run state"),
                }
            }
        }
    }
    let _ = stop.send(());
    // Cancellation is persisted before cooperative messages are sent to children.
    if service.begin_shutdown().await.is_err() {
        eprintln!("shutdown intent could not be fully persisted");
    }
    supervisor.drain();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !service.runs_drained() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    if !recovery_done {
        recovery.abort();
        let _ = recovery.await;
    }
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
        } else {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
