use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
mod catalog;
mod schedule;
mod voice;

use api::{Api, LifecycleIntent, LocalAddress, Services};
use chrono::{SecondsFormat, Utc};
use filesystem::local::{
    checkout::LocalCheckout, forge::LocalForge, github_projects::LocalGithubProjects,
    provisioning::LocalDirectorySource, workspace_runtime::LocalWorkspaceRuntime,
    worktrees::LocalManagedWorktrees,
};
use filesystem::service::checkout::Checkout;
use filesystem::service::files::Files;
use filesystem::service::forge::Forge;
use filesystem::service::worktrees::{WorkspaceWorktrees, Worktrees};
use filesystem::service::{github_projects::GithubProjects, workspace_recovery::WorkspaceRecovery};
use metadata::local::workspace_automation::LocalWorkspaceAutomation;
use metadata::ports::generation::MetadataGenerator;
use metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use metadata::service::daemon::{Daemon, DaemonRuntime};
use metadata::service::directory::{Directory, DirectoryDependencies};
use metadata::service::workspace_automation::WorkspaceAutomation;
use metadata::service::workspace_labels::WorkspaceLabels;
use metadata::service::workspace_names::WorkspaceNames;
use metadata::service::workspace_state::WorkspaceState;
use metadata::storage::daemon_config::FileDaemonConfigStore;
use metadata::storage::project_config::LocalProjectConfigStore;
use metadata::storage::project_icon::LocalProjectIconStore;
use metadata::storage::registry::{FileBackedProjectRegistry, FileBackedWorkspaceRegistry};
use metadata::storage::workspace_labels::FileWorkspaceLabelStore;
use provider::local::{claude::ClaudeClient, codex::CodexClient};
use provider::ports::agent_runtime::AgentRuntimeRegistry;
use provider::service::agent_execution::{AgentExecution, ExecutionDependencies};
use provider::service::agent_manager::AgentManager;
use provider::service::agent_runtime::AgentRuntimeDirectory;
use provider::service::agents::Agents;
use provider::service::workspace_attention::AgentWorkspaceAttention;
use provider::storage::SqliteCatalog;
use provider::storage::agent_runtime::FileBackedAgentRuntimeRegistry;

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
        let web_origins = config.web_origins.clone();
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
        )?
        .with_browser_origins(web_origins)?;
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
            let server = axum::serve(
                listener,
                api.router()
                    .into_make_service_with_connect_info::<LocalAddress>(),
            )
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
    let (project_registry, workspace_registry) = open_directory_registries(config)?;
    let changes = model::changes::Changes::default();
    let agent_runtime_registry =
        FileBackedAgentRuntimeRegistry::new(config.data_dir.join("agents/agents.json"))
            .with_changes(changes.clone());
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
    let MetadataServices {
        config: config_store,
        generator: metadata_generator,
        names: workspace_names,
    } = compose_metadata(&config.data_dir, &workspace_registry);
    let worktrees = Arc::new(Mutex::new(
        compose_worktrees(config, &project_registry, &workspace_registry, &server_id)
            .with_workspace_names(workspace_names.clone()),
    ));
    let WorkspaceServices {
        automation: workspace_automation,
        state: workspace_state,
        recovery: workspace_recovery,
    } = compose_workspace_services(
        config,
        &workspace_registry,
        &project_registry,
        &agent_runtime_registry,
    );
    let daemon = compose_daemon(config_store, address, &server_id)?;
    let workspace_automation = Arc::new(Mutex::new(workspace_automation));
    let timeline = open_timeline(&config.data_dir)?;
    let terminals = compose_terminals(&workspace_registry, &project_registry, &changes);
    let directory = compose_directory(
        config,
        &project_registry,
        &workspace_registry,
        server_id,
        changes,
    )?
    .with_worktrees(Arc::new(WorkspaceWorktrees::new(worktrees.clone())))
    .with_workspace_names(workspace_names.clone())
    .with_activity_source(Arc::new(
        AgentWorkspaceAttention::new(Box::new(agent_runtime_registry.clone()))
            .with_timeline(timeline.clone()),
    ))
    .with_activity_source(Arc::new(terminals.activity_source()));
    let agent_execution = compose_provider(
        (agent_runtime_registry, timeline),
        (&workspace_registry, &project_registry),
        instance,
        &config.data_dir,
        (
            directory.clone(),
            metadata_generator.clone(),
            workspace_names.clone(),
            workspace_automation.clone(),
        ),
    )?;
    let schedules = schedule::compose(
        &config.data_dir,
        agent_execution.clone(),
        directory.clone(),
        worktrees.clone(),
    )?;
    let github_projects =
        GithubProjects::new(directory.clone(), Box::new(LocalGithubProjects::new()));
    let (checkout, git_fetch) = compose_git(&config.data_dir);
    Ok(Services {
        metadata_generator: Some(metadata_generator),
        workspace_names: Some(workspace_names),
        schedules: Some(schedules),
        browser: Some(browser::broker::Broker::default()),
        skills: Some(compose_skills(&config.data_dir)?),
        push_tokens: Some(compose_push(&config.data_dir)?),
        speech: Some(voice::compose(agent_execution.clone(), &config.data_dir)?),
        terminals: Some(terminals),
        agent_execution: Some(agent_execution),
        agents: Some(agents),
        checkout: Some(checkout),
        git_fetch: Some(git_fetch),
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

fn open_directory_registries(
    config: &Config,
) -> anyhow::Result<(FileBackedProjectRegistry, FileBackedWorkspaceRegistry)> {
    let projects = FileBackedProjectRegistry::new(config.data_dir.join("projects/projects.json"));
    let workspaces =
        FileBackedWorkspaceRegistry::new(config.data_dir.join("projects/workspaces.json"));
    projects.initialize()?;
    workspaces.initialize()?;
    Ok((projects, workspaces))
}

fn compose_terminals(
    workspaces: &FileBackedWorkspaceRegistry,
    projects: &FileBackedProjectRegistry,
    changes: &model::changes::Changes,
) -> terminal::service::Terminals {
    let mut terminals = terminal::service::Terminals::new(
        Box::new(workspaces.clone()),
        Box::new(projects.clone()),
        Box::new(terminal::local::LocalRuntime),
    );
    terminals.set_directory_changes(changes.clone());
    terminals
}

fn compose_git(data_dir: &std::path::Path) -> (Checkout, filesystem::service::git_fetch::GitFetch) {
    let root = data_dir.join("worktrees");
    let checkout = Checkout::new(Box::new(LocalCheckout::new(root.clone())));
    let fetch = filesystem::service::git_fetch::GitFetch::new(Arc::new(
        filesystem::local::git_fetch::LocalGitFetch::new(root),
    ));
    (checkout, fetch)
}

struct WorkspaceServices {
    automation: WorkspaceAutomation,
    state: WorkspaceState,
    recovery: WorkspaceRecovery,
}

fn compose_workspace_services(
    config: &Config,
    workspace_registry: &FileBackedWorkspaceRegistry,
    project_registry: &FileBackedProjectRegistry,
    agent_runtime_registry: &FileBackedAgentRuntimeRegistry,
) -> WorkspaceServices {
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
    WorkspaceServices {
        automation: workspace_automation,
        state: workspace_state,
        recovery: workspace_recovery,
    }
}

struct MetadataServices {
    config: FileDaemonConfigStore,
    generator: Arc<dyn MetadataGenerator>,
    names: WorkspaceNames,
}

fn compose_metadata(
    data_dir: &std::path::Path,
    registry: &FileBackedWorkspaceRegistry,
) -> MetadataServices {
    let config_store = FileDaemonConfigStore::with_defaults(data_dir.join("config.json"));
    let (codex, claude) = native_clients(data_dir);
    let metadata_generator: Arc<dyn MetadataGenerator> =
        Arc::new(provider::service::metadata_generation::Generation::new(
            Arc::new(config_store.clone()),
            vec![Arc::new(codex), Arc::new(claude)],
        ));
    let workspace_names = WorkspaceNames::new(
        Arc::new(registry.clone()),
        metadata_generator.clone(),
        Arc::new(LocalCheckout::new(data_dir.join("worktrees"))),
    );
    MetadataServices {
        config: config_store,
        generator: metadata_generator,
        names: workspace_names,
    }
}

fn compose_directory(
    config: &Config,
    project_registry: &FileBackedProjectRegistry,
    workspace_registry: &FileBackedWorkspaceRegistry,
    server_id: String,
    changes: model::changes::Changes,
) -> anyhow::Result<Directory> {
    let creations = metadata::service::creation::Creations::open(
        config.data_dir.join("creations/receipts.json"),
    )
    .map_err(|_| anyhow::anyhow!("initialize creation receipts"))?;
    Ok(Directory::new(DirectoryDependencies {
        projects: Box::new(project_registry.clone()),
        workspaces: Box::new(workspace_registry.clone()),
        source: Box::new(LocalDirectorySource),
        config_store: Box::new(LocalProjectConfigStore),
        icon_store: Box::new(LocalProjectIconStore::new(
            config.data_dir.join("projects/icons"),
        )),
        server_id,
    })
    .with_changes(changes.clone())
    .with_creations(creations)
    .with_runtime_source(Arc::new(
        LocalWorkspaceRuntime::new(
            LocalCheckout::new(config.data_dir.join("worktrees")),
            LocalForge::new(),
        )
        .with_changes(changes),
    )))
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

fn compose_daemon(
    config_store: FileDaemonConfigStore,
    address: SocketAddr,
    server_id: &str,
) -> anyhow::Result<Daemon> {
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
        Box::new(config_store),
    );
    daemon.get_config().context("initialize daemon config")?;
    Ok(daemon)
}

fn open_timeline(
    data_dir: &std::path::Path,
) -> anyhow::Result<provider::storage::timeline::Timeline> {
    provider::storage::timeline::Timeline::open(&data_dir.join("agents/timeline.sqlite3"))
        .map_err(|_| anyhow::anyhow!("initialize Agent timeline"))
}

fn compose_provider(
    agent_storage: (
        FileBackedAgentRuntimeRegistry,
        provider::storage::timeline::Timeline,
    ),
    registries: (&FileBackedWorkspaceRegistry, &FileBackedProjectRegistry),
    instance: &Arc<InstanceLease>,
    data_dir: &std::path::Path,
    metadata: (
        Directory,
        Arc<dyn MetadataGenerator>,
        WorkspaceNames,
        Arc<Mutex<WorkspaceAutomation>>,
    ),
) -> anyhow::Result<AgentExecution> {
    let (directory, generator, names, workspace_automation) = metadata;
    let (agent_runtime_registry, timeline) = agent_storage;
    let (workspace_registry, project_registry) = registries;
    let mut manager = AgentManager::new(Box::new(agent_runtime_registry.clone()))
        .with_timeline(timeline)
        .with_creations(directory.creations())
        .with_metadata_generation(generator)
        .with_workspace_names(names);
    let (codex, claude) = native_clients(data_dir);
    manager.register_client(Box::new(codex))?;
    manager.register_client(Box::new(claude))?;
    manager.register_client(Box::new(provider::local::opencode::OpenCodeClient::new(
        std::env::var_os("AIT_SERVER_OPENCODE_BIN").map_or_else(|| "opencode".into(), Into::into),
    )))?;
    manager.register_client(Box::new(
        provider::local::deepseek_harness::DeepSeekHarnessClient::new(
            std::env::var_os("AIT_SERVER_DEEPSEEK_HARNESS_BIN")
                .map_or_else(|| "dsh".into(), Into::into),
        )
        .with_image_directory(data_dir.join("agents/provider-images")),
    ))?;
    AgentExecution::spawn(ExecutionDependencies {
        manager,
        directory: AgentRuntimeDirectory::new(
            Box::new(agent_runtime_registry.clone()),
            Box::new(workspace_registry.clone()),
            Box::new(project_registry.clone()),
        )
        .with_directory_sync(directory.directory_sync()),
        registry: Box::new(agent_runtime_registry),
        workspaces: Box::new(workspace_registry.clone()),
        lifetime: instance.clone(),
        import_directory: Some(directory),
        workspace_automation: Some(workspace_automation),
        projects: Box::new(project_registry.clone()),
    })
    .context("start Provider worker")
}

fn native_clients(data_dir: &std::path::Path) -> (CodexClient, ClaudeClient) {
    (
        CodexClient::new(
            std::env::var_os("AIT_SERVER_CODEX_BIN").map_or_else(|| "codex".into(), Into::into),
        )
        .with_image_directory(data_dir.join("agents/provider-images")),
        ClaudeClient::new(
            std::env::var_os("AIT_SERVER_CLAUDE_BIN").map_or_else(|| "claude".into(), Into::into),
        )
        .with_image_directory(data_dir.join("agents/provider-images")),
    )
}

#[cfg(test)]
mod tests;

fn compose_push(data_dir: &std::path::Path) -> anyhow::Result<metadata::service::push::PushTokens> {
    metadata::service::push::PushTokens::open(
        Box::new(metadata::storage::push::FileTokenStore::new(
            data_dir.join("push-tokens.json"),
        )),
        chrono::Utc::now().timestamp_millis(),
    )
    .map_err(|_| anyhow::anyhow!("initialize push token leases"))
}

fn compose_skills(data: &std::path::Path) -> anyhow::Result<filesystem::service::skills::Skills> {
    let data = data
        .canonicalize()
        .context("resolve skills data directory")?;
    let home = std::env::var_os("AIT_SERVER_SKILLS_HOME")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| data.join("agent-home"), std::path::PathBuf::from);
    let source = std::env::var_os("AIT_SERVER_SKILLS_BUNDLE")
        .map_or_else(|| data.join("skills-bundle"), std::path::PathBuf::from);
    let targets = [".agents/skills", ".claude/skills", ".codex/skills"].map(|path| home.join(path));
    let store =
        filesystem::local::skills::LocalSkills::new(&source, &targets, &data.join("skills-state"))
            .map_err(|error| anyhow::anyhow!("invalid skills configuration: {error:?}"))?;
    Ok(filesystem::service::skills::Skills::new(Box::new(store)))
}

fn compose_files(config: &Config) -> Files {
    Files::new(Box::new(filesystem::local::files::LocalFiles::new(
        std::env::var_os("HOME").map_or_else(|| config.data_dir.clone(), std::path::PathBuf::from),
        &config.data_dir,
    )))
}
