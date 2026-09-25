use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
mod catalog;
mod schedule;
mod voice;

use chrono::{SecondsFormat, Utc};
use server_api::{Api, LifecycleIntent, Services};
use server_filesystem::local::{
    checkout::LocalCheckout, forge::LocalForge, github_projects::LocalGithubProjects,
    provisioning::LocalDirectorySource, worktrees::LocalManagedWorktrees,
};
use server_filesystem::service::checkout::Checkout;
use server_filesystem::service::files::Files;
use server_filesystem::service::forge::Forge;
use server_filesystem::service::worktrees::Worktrees;
use server_filesystem::service::{
    github_projects::GithubProjects, workspace_recovery::WorkspaceRecovery,
};
use server_metadata::local::workspace_automation::LocalWorkspaceAutomation;
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use server_metadata::service::daemon::{Daemon, DaemonRuntime};
use server_metadata::service::directory::{Directory, DirectoryDependencies};
use server_metadata::service::workspace_automation::WorkspaceAutomation;
use server_metadata::service::workspace_labels::WorkspaceLabels;
use server_metadata::service::workspace_state::WorkspaceState;
use server_metadata::storage::daemon_config::FileDaemonConfigStore;
use server_metadata::storage::project_config::LocalProjectConfigStore;
use server_metadata::storage::project_icon::LocalProjectIconStore;
use server_metadata::storage::registry::{FileBackedProjectRegistry, FileBackedWorkspaceRegistry};
use server_metadata::storage::workspace_labels::FileWorkspaceLabelStore;
use server_provider::local::codex::CodexClient;
use server_provider::ports::agent_runtime::AgentRuntimeRegistry;
use server_provider::service::agent_execution::{AgentExecution, ExecutionDependencies};
use server_provider::service::agent_manager::AgentManager;
use server_provider::service::agent_runtime::AgentRuntimeDirectory;
use server_provider::service::agents::Agents;
use server_provider::service::workspace_attention::AgentWorkspaceAttention;
use server_provider::storage::SqliteCatalog;
use server_provider::storage::agent_runtime::FileBackedAgentRuntimeRegistry;

use tokio::net::TcpListener;

use crate::config::Config;
use crate::instance::InstanceLease;

pub(super) struct Server {
    listener: TcpListener,
    api: Api,
    // Own the directory lock until all accepted connections have stopped.
    instance: Arc<InstanceLease>,
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
    let server_id = instance.server_id.to_string();
    let worktrees = compose_worktrees(config, &project_registry, &workspace_registry, &server_id);
    let workspace_automation = WorkspaceAutomation::new(
        Box::new(workspace_registry.clone()),
        Box::new(LocalWorkspaceAutomation::default()),
    );
    let workspace_state = WorkspaceState::new(
        Box::new(AgentWorkspaceAttention::new(Box::new(
            agent_runtime_registry.clone(),
        ))),
        Box::new(workspace_registry.clone()),
    );
    let workspace_recovery = WorkspaceRecovery::new(
        Box::new(workspace_registry.clone()),
        Box::new(project_registry.clone()),
        Box::new(LocalManagedWorktrees::new(
            config.data_dir.join("worktrees"),
        )),
    );
    let daemon = compose_daemon(config, address, &server_id)?;
    let creations = server_metadata::service::creation::Creations::open(
        config.data_dir.join("creations/receipts.json"),
    )
    .map_err(|_| anyhow::anyhow!("initialize creation receipts"))?;
    let directory = compose_directory(
        config,
        &project_registry,
        &workspace_registry,
        server_id,
        creations,
    );
    let agent_execution = compose_provider(
        agent_runtime_registry,
        (&workspace_registry, &project_registry),
        instance,
        &config.data_dir,
        directory.clone(),
    )?;
    let schedules = schedule::compose(
        &config.data_dir,
        agent_execution.clone(),
        directory.clone(),
        compose_worktrees(
            config,
            &project_registry,
            &workspace_registry,
            &instance.server_id.to_string(),
        ),
    )?;
    let github_projects =
        GithubProjects::new(directory.clone(), Box::new(LocalGithubProjects::new()));
    Ok(Services {
        schedules: Some(schedules),
        browser: Some(server_browser::broker::Broker::default()),
        skills: Some(compose_skills(&config.data_dir)?),
        push_tokens: Some(compose_push(&config.data_dir)?),
        speech: Some(voice::compose(agent_execution.clone())?),
        terminals: Some(server_terminal::service::Terminals::new(
            Box::new(workspace_registry.clone()),
            Box::new(project_registry.clone()),
            Box::new(server_terminal::local::LocalRuntime),
        )),
        agent_execution: Some(agent_execution),
        agents: Some(agents),
        checkout: Some(Checkout::new(Box::new(LocalCheckout::new(
            config.data_dir.join("worktrees"),
        )))),
        agent_runtime: None,
        daemon: Some(daemon),
        directory: Some(directory),
        github_projects: Some(github_projects),
        workspace_recovery: Some(workspace_recovery),
        forge: Some(Forge::new(Box::new(LocalForge::new()))),
        files: Some(compose_files(config)),
        workspace_labels: Some(workspace_labels),
        workspace_automation: Some(workspace_automation),
        workspace_state: Some(workspace_state),
        worktrees: Some(worktrees),
    })
}

fn compose_directory(
    config: &Config,
    project_registry: &FileBackedProjectRegistry,
    workspace_registry: &FileBackedWorkspaceRegistry,
    server_id: String,
    creations: server_metadata::service::creation::Creations,
) -> Directory {
    Directory::new(DirectoryDependencies {
        projects: Box::new(project_registry.clone()),
        workspaces: Box::new(workspace_registry.clone()),
        source: Box::new(LocalDirectorySource),
        config_store: Box::new(LocalProjectConfigStore),
        icon_store: Box::new(LocalProjectIconStore::new(
            config.data_dir.join("projects/icons"),
        )),
        server_id,
    })
    .with_creations(creations)
}

fn compose_worktrees(
    config: &Config,
    projects: &FileBackedProjectRegistry,
    workspaces: &FileBackedWorkspaceRegistry,
    server_id: &str,
) -> Worktrees {
    Worktrees::new(
        Box::new(projects.clone()),
        Box::new(workspaces.clone()),
        Box::new(LocalManagedWorktrees::new(
            config.data_dir.join("worktrees"),
        )),
        server_id.to_owned(),
    )
}

fn compose_daemon(config: &Config, address: SocketAddr, server_id: &str) -> anyhow::Result<Daemon> {
    let daemon = Daemon::new(
        DaemonRuntime {
            server_id: server_id.to_owned(),
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
    Ok(daemon)
}

fn compose_provider(
    agent_runtime_registry: FileBackedAgentRuntimeRegistry,
    registries: (&FileBackedWorkspaceRegistry, &FileBackedProjectRegistry),
    instance: &Arc<InstanceLease>,
    data_dir: &std::path::Path,
    directory: Directory,
) -> anyhow::Result<AgentExecution> {
    let (workspace_registry, project_registry) = registries;
    let timeline = server_provider::storage::timeline::Timeline::open(
        &data_dir.join("agents/timeline.sqlite3"),
    )
    .map_err(|_| anyhow::anyhow!("initialize Agent timeline"))?;
    let mut manager = AgentManager::new(Box::new(agent_runtime_registry.clone()))
        .with_timeline(timeline)
        .with_creations(directory.creations());
    manager.register_client(Box::new(CodexClient::new(
        std::env::var_os("AIT_SERVER_CODEX_BIN").map_or_else(|| "codex".into(), Into::into),
    )))?;
    AgentExecution::spawn(ExecutionDependencies {
        manager,
        directory: AgentRuntimeDirectory::new(
            Box::new(agent_runtime_registry.clone()),
            Box::new(workspace_registry.clone()),
            Box::new(project_registry.clone()),
        ),
        registry: Box::new(agent_runtime_registry),
        workspaces: Box::new(workspace_registry.clone()),
        lifetime: instance.clone(),
        import_directory: Some(directory),
        projects: Box::new(project_registry.clone()),
    })
    .context("start Provider worker")
}

#[cfg(test)]
mod tests;

fn compose_push(
    data_dir: &std::path::Path,
) -> anyhow::Result<server_metadata::service::push::PushTokens> {
    server_metadata::service::push::PushTokens::open(
        Box::new(server_metadata::storage::push::FileTokenStore::new(
            data_dir.join("push-tokens.json"),
        )),
        chrono::Utc::now().timestamp_millis(),
    )
    .map_err(|_| anyhow::anyhow!("initialize push token leases"))
}

fn compose_skills(
    data: &std::path::Path,
) -> anyhow::Result<server_filesystem::service::skills::Skills> {
    let data = data
        .canonicalize()
        .context("resolve skills data directory")?;
    let home = std::env::var_os("AIT_SERVER_SKILLS_HOME")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| data.join("agent-home"), std::path::PathBuf::from);
    let source = std::env::var_os("AIT_SERVER_SKILLS_BUNDLE")
        .map_or_else(|| data.join("skills-bundle"), std::path::PathBuf::from);
    let targets = [".agents/skills", ".claude/skills", ".codex/skills"].map(|path| home.join(path));
    let store = server_filesystem::local::skills::LocalSkills::new(
        &source,
        &targets,
        &data.join("skills-state"),
    )
    .map_err(|error| anyhow::anyhow!("invalid skills configuration: {error:?}"))?;
    Ok(server_filesystem::service::skills::Skills::new(Box::new(
        store,
    )))
}

fn compose_files(config: &Config) -> Files {
    Files::new(Box::new(server_filesystem::local::files::LocalFiles::new(
        std::env::var_os("HOME").map_or_else(|| config.data_dir.clone(), std::path::PathBuf::from),
        &config.data_dir,
    )))
}
