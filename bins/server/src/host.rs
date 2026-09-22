use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
mod catalog;

use chrono::{SecondsFormat, Utc};
use server_api::{Api, LifecycleIntent, Services};
use server_application::Projects;
use server_application::agent_runtime::AgentRuntimeDirectory;
use server_application::agents::Agents;
use server_application::daemon::{Daemon, DaemonRuntime};
use server_application::directory::Directory;
use server_application::workspace_automation::WorkspaceAutomation;
use server_application::workspace_labels::WorkspaceLabels;
use server_application::worktrees::Worktrees;
use server_ports::agent_runtime::AgentRuntimeRegistry;
use server_ports::registry::{ProjectRegistry, WorkspaceRegistry};
use server_ports::{ProjectError, ProjectStorage, ProjectStore};
use server_storage::daemon_config::FileDaemonConfigStore;
use server_storage::registry::{
    FileBackedAgentRuntimeRegistry, FileBackedProjectRegistry, FileBackedWorkspaceRegistry,
};
use server_storage::workspace_labels::FileWorkspaceLabelStore;
use server_storage::{SqliteCatalog, SqliteProjects};
use server_workspace::{
    LocalDirectorySource, LocalManagedWorktrees, LocalProjectConfigStore, LocalProjectIconStore,
    LocalWorkspace, LocalWorkspaceAutomation,
};
use tokio::net::TcpListener;

use crate::config::Config;
use crate::instance::InstanceLease;

pub(super) struct Server {
    listener: TcpListener,
    api: Api,
    // Own the directory lock until all accepted connections have stopped.
    instance: Arc<InstanceLease>,
}

#[derive(Debug)]
struct OwnedStorage {
    // A timed-out or abandoned response must not release the catalog's process lease
    // while its supervised blocking job can still write. Projects owns this factory
    // until after its open stores and catalog are dropped.
    _instance: Arc<InstanceLease>,
}

impl ProjectStorage for OwnedStorage {
    fn open(&self, root: &Path) -> Result<Box<dyn ProjectStore>, ProjectError> {
        SqliteProjects.open(root)
    }
}

impl Server {
    pub async fn bind(config: Config) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(config.listen)
            .await
            .context("bind server listener")?;
        let token = config.token.clone();
        let address = listener
            .local_addr()
            .context("read server listener address")?;
        let (instance, services) = tokio::task::spawn_blocking(move || {
            let instance = Arc::new(InstanceLease::acquire(&config.data_dir)?);
            let services = compose_services(&config, address, &instance)?;
            Ok::<_, anyhow::Error>((instance, services))
        })
        .await
        .context("join server initialization")??;
        let api = Api::new(
            address,
            instance.server_id.to_string(),
            instance.instance_id.to_string(),
            token,
            services,
        )?;
        Ok(Self {
            listener,
            api,
            instance,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound TCP listener has a local address")
    }

    pub async fn serve(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<Option<LifecycleIntent>> {
        let Self {
            listener,
            api,
            instance,
        } = self;
        let result: anyhow::Result<()> = async {
            let shutdown_api = api.clone();
            let server = axum::serve(listener, api.router())
                .with_graceful_shutdown(async move {
                    tokio::select! {
                        () = shutdown => {},
                        () = shutdown_api.wait_draining() => {},
                    }
                    shutdown_api.begin_shutdown();
                })
                .into_future();
            tokio::pin!(server);
            // Readiness changes in the signal future before HTTP acceptance stops.
            // Also clean up WS tasks if the HTTP server terminates with an error.
            let result = tokio::select! {
                result = &mut server => Some(result),
                () = api.wait_draining() => None,
            };
            api.begin_shutdown();
            tokio::time::timeout(Duration::from_secs(15), async {
                let result = match result {
                    Some(result) => result,
                    None => server.await,
                };
                api.wait_closed().await;
                result.context("serve HTTP")
            })
            .await
            .context("server shutdown exceeded 15 seconds")??;
            Ok(())
        }
        .await;
        let lifecycle_intent = api.lifecycle_intent();
        // Drop routers and application storage before the data-directory instance lease.
        drop(api);
        drop(instance);
        result?;
        Ok(lifecycle_intent)
    }
}

fn compose_services(
    config: &Config,
    address: SocketAddr,
    instance: &Arc<InstanceLease>,
) -> anyhow::Result<Services> {
    let project_registry =
        FileBackedProjectRegistry::new(config.data_dir.join("projects/projects.json"));
    let workspace_registry =
        FileBackedWorkspaceRegistry::new(config.data_dir.join("projects/workspaces.json"));
    project_registry.initialize()?;
    workspace_registry.initialize()?;
    let agent_runtime_registry =
        FileBackedAgentRuntimeRegistry::new(config.data_dir.join("agents/agents.json"));
    agent_runtime_registry.initialize()?;
    let workspace_labels = WorkspaceLabels::new(Box::new(FileWorkspaceLabelStore::new(
        &config.data_dir,
        workspace_registry.clone(),
    )))?;
    let agents = Agents::new(Box::new(catalog::OwnedCatalog {
        catalog: SqliteCatalog::open(&config.data_dir)?,
        _instance: instance.clone(),
    }));
    let projects = Projects::new(
        Box::new(SqliteCatalog::open(&config.data_dir)?),
        Box::new(OwnedStorage {
            _instance: instance.clone(),
        }),
        Box::new(LocalWorkspace::for_user()?),
    );
    let server_id = instance.server_id.to_string();
    let worktrees = Worktrees::new(
        Box::new(project_registry.clone()),
        Box::new(workspace_registry.clone()),
        Box::new(LocalManagedWorktrees::new(
            config.data_dir.join("worktrees"),
        )),
        server_id.clone(),
    );
    let workspace_automation = WorkspaceAutomation::new(
        Box::new(workspace_registry.clone()),
        Box::new(LocalWorkspaceAutomation::default()),
    );
    let daemon = Daemon::new(
        DaemonRuntime {
            server_id: server_id.clone(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            pid: std::process::id(),
            executable: std::env::current_exe()
                .context("resolve server executable")?
                .to_string_lossy()
                .into_owned(),
            started_at: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
            listen: address.to_string(),
        },
        Box::new(FileDaemonConfigStore::with_defaults(
            config.data_dir.join("config.json"),
        )),
    );
    daemon.get_config().context("initialize daemon config")?;
    Ok(Services {
        projects: Some(projects),
        agents: Some(agents),
        agent_runtime: Some(AgentRuntimeDirectory::new(
            Box::new(agent_runtime_registry),
            Box::new(workspace_registry.clone()),
            Box::new(project_registry.clone()),
        )),
        daemon: Some(daemon),
        directory: Some(Directory::new(
            Box::new(project_registry),
            Box::new(workspace_registry),
            Box::new(LocalDirectorySource),
            Box::new(LocalProjectConfigStore),
            Box::new(LocalProjectIconStore::new(
                config.data_dir.join("projects/icons"),
            )),
            server_id,
        )),
        workspace_labels: Some(workspace_labels),
        workspace_automation: Some(workspace_automation),
        worktrees: Some(worktrees),
    })
}

#[cfg(test)]
mod tests;
