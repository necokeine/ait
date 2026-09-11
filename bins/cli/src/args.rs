//! Discoverable, entity-scoped CLI arguments and their application command mapping.

use std::{
    io::{self, Read},
    path::PathBuf,
};

use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, Command, NativeApprovalAction, ProviderSecret,
};
use ait_domain::ApprovalGrantScope;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::input::{self, StdinSource};

#[derive(Parser)]
#[command(
    name = "ait",
    about = "Manage Projects, Agents and Runs through the local daemon"
)]
pub(crate) struct Arguments {
    /// Local daemon HTTP endpoint (valid at every subcommand level).
    #[arg(long, global = true, default_value = "http://127.0.0.1:7314", value_parser = input::url)]
    pub(crate) endpoint: reqwest::Url,
    #[command(subcommand)]
    pub(crate) command: CliCommand,
}

#[derive(Subcommand)]
pub(crate) enum CliCommand {
    /// Manage Project operations.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Manage Agent operations.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Manage Provider operations.
    AgentProvider {
        #[command(subcommand)]
        command: AgentProviderCommand,
    },
    /// Manage Session operations.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Manage Message operations.
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Manage Run operations.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Manage Cron operations.
    Cron {
        #[command(subcommand)]
        command: CronCommand,
    },
    /// Manage Settings operations.
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    /// Manage Event operations.
    Event {
        #[command(subcommand)]
        command: EventCommand,
    },
    /// Replay durable SSE after a cursor (also: event list).
    Events {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    /// Export a Project archive to a file (also: project export).
    Export(ExportArgs),
    /// Import a Project archive (also: project import).
    Import(ImportArgs),
}

#[derive(Args)]
pub(crate) struct ExportArgs {
    #[arg(long, value_parser = input::id)]
    project_id: String,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
pub(crate) struct ImportArgs {
    /// Project archive JSON file, or - for stdin.
    #[arg(long, value_name = "FILE|-")]
    input: PathBuf,
    #[arg(long)]
    workdir: PathBuf,
}

#[derive(Subcommand)]
pub(crate) enum ProjectCommand {
    /// List Projects.
    List,
    /// Register a workdir and establish its Git baseline.
    Register {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        workdir: PathBuf,
        #[arg(long)]
        repo_url: Option<String>,
    },
    /// Set the suggested Agent for new Sessions.
    SetDefaultAgent {
        #[arg(long, value_parser = input::id)]
        project_id: String,
        #[arg(long, value_parser = input::id)]
        agent_id: String,
    },
    /// Export all branches and Session refs to a file.
    Export(ExportArgs),
    /// Import an archive into an explicit local workdir.
    Import(ImportArgs),
}

#[derive(Subcommand)]
pub(crate) enum AgentCommand {
    /// List Agents.
    List,
    /// Create a reusable Agent.
    Create {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        config: Config,
    },
    /// Replace an Agent configuration; existing Runs retain their snapshot.
    Update {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        config: Config,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentProviderCommand {
    /// List Providers without credentials.
    List,
    /// Refresh models using the saved connection.
    RefreshModels {
        #[arg(long, value_parser = input::id)]
        provider_id: String,
    },
    /// Save connection metadata and optionally a write-only secret.
    Save(ProviderArgs),
    /// Discover models without saving the connection.
    DiscoverModels(ProviderArgs),
}

#[derive(Subcommand)]
pub(crate) enum SessionCommand {
    /// List Sessions in one Project.
    List {
        #[arg(long, value_parser = input::id)]
        project_id: String,
    },
    /// Create a Session at the Project root or an existing Message.
    Create {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long, value_parser = input::id)]
        project_id: String,
        #[arg(long, value_parser = input::id)]
        agent_id: String,
        #[arg(long, value_parser = input::id)]
        at_message_id: Option<String>,
    },
    /// Rebind an idle Session to an Agent.
    SetAgent {
        #[arg(long, value_parser = input::id)]
        session_id: String,
        #[arg(long, value_parser = input::id)]
        agent_id: String,
    },
    /// Configure the Session-owned Agent.
    SetConfig {
        #[arg(long, value_parser = input::id)]
        session_id: String,
        #[command(flatten)]
        config: Config,
    },
    /// Set the manual Session name (empty clears it).
    Rename {
        #[arg(long, value_parser = input::id)]
        session_id: String,
        #[arg(long)]
        name: String,
    },
    /// Set the Session title.
    SetTitle {
        #[arg(long, value_parser = input::id)]
        session_id: String,
        #[arg(long)]
        title: String,
    },
    /// Send input. Check the returned Run status and error; ok=true does not mean completed.
    Send {
        #[arg(long, value_parser = input::id)]
        session_id: String,
        #[command(flatten)]
        text: TextInput,
    },
    /// Create a branch and send input. Check Run status and error.
    Fork {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long, value_parser = input::id)]
        project_id: String,
        #[arg(long, value_parser = input::id)]
        agent_id: String,
        #[arg(long, value_parser = input::id)]
        at_message_id: String,
        #[command(flatten)]
        text: TextInput,
    },
    /// Atomically reuse or fork a Session and send input. Check Run status and error.
    Derive {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long, value_parser = input::id)]
        project_id: String,
        #[arg(long, value_parser = input::id)]
        source_session_id: String,
        #[arg(long, value_parser = input::id)]
        agent_id: String,
        #[arg(long, value_parser = input::id)]
        at_message_id: String,
        #[command(flatten)]
        text: TextInput,
    },
}

#[derive(Subcommand)]
pub(crate) enum MessageCommand {
    /// List immutable Messages in one Project.
    List {
        #[arg(long, value_parser = input::id)]
        project_id: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum RunCommand {
    /// List Runs in one Project.
    List {
        #[arg(long, value_parser = input::id)]
        project_id: String,
    },
    /// Read Run status, error and pending native approvals.
    Get {
        #[arg(long, value_parser = input::id)]
        run_id: String,
    },
    /// Cancel a Run; a terminal Run is a business error.
    Cancel {
        #[arg(long, value_parser = input::id)]
        run_id: String,
    },
    /// Resolve a pending native approval on the same Run.
    Approval {
        #[command(subcommand)]
        command: ApprovalCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum CronCommand {
    /// List Crons.
    List,
    /// Create a Cron with a fixed Message and Agent.
    Create {
        #[arg(long, value_parser = input::id)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = input::id)]
        project_id: String,
        #[arg(long, value_parser = input::id)]
        base_message_id: String,
        #[arg(long, value_parser = input::id)]
        agent_id: String,
        #[arg(long)]
        schedule: String,
        #[arg(long)]
        timezone: String,
    },
    /// Enable a Cron.
    Enable {
        #[arg(long, value_parser = input::id)]
        cron_id: String,
    },
    /// Disable a Cron.
    Disable {
        #[arg(long, value_parser = input::id)]
        cron_id: String,
    },
    /// Trigger an occurrence. scheduled-at is Unix milliseconds (signed i64); the same value deduplicates.
    Trigger {
        #[arg(long, value_parser = input::id)]
        cron_id: String,
        #[arg(long, allow_hyphen_values = true)]
        scheduled_at: i64,
    },
}

#[derive(Subcommand)]
pub(crate) enum SettingsCommand {
    /// Read the settings schema, full values and revision.
    Get,
    /// Restore defaults: `read_only` sandbox and `on_request` approval.
    Reset,
    /// Replace the complete settings document using its observed revision.
    Set {
        #[arg(long)]
        expected_revision: u64,
        /// Full values object keyed by setting name; no envelope or revision inside.
        #[arg(long, value_name = "FILE|-")]
        input: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum EventCommand {
    /// Replay durable SSE strictly after the cursor (default page: 256 events).
    List {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
}

#[derive(Args)]
pub(crate) struct Config {
    #[arg(long, value_parser = input::id)]
    provider_id: String,
    #[arg(long, value_parser = input::id)]
    model: String,
    /// Provider/model-specific effort from agent-provider list (validated by the daemon).
    #[arg(long, value_parser = input::id)]
    reasoning_effort: Option<String>,
}

impl From<Config> for AgentConfiguration {
    fn from(value: Config) -> Self {
        Self {
            provider_id: value.provider_id,
            model: value.model,
            reasoning_effort: value.reasoning_effort,
        }
    }
}

#[derive(Args)]
#[group(required = true, multiple = false)]
pub(crate) struct TextInput {
    /// Literal message text, including newlines.
    #[arg(long)]
    text: Option<String>,
    /// UTF-8 text file; preserves newlines and backslashes exactly. Use - for stdin.
    #[arg(long, value_name = "FILE|-")]
    text_file: Option<PathBuf>,
    /// Read UTF-8 message text from stdin, preserving all bytes.
    #[arg(long)]
    text_stdin: bool,
}

impl TextInput {
    fn read(self, stdin: &mut dyn Read) -> Result<String, io::Error> {
        if let Some(text) = self.text {
            return Ok(text);
        }
        input::read(&self.text_file.unwrap_or_else(|| PathBuf::from("-")), stdin)
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ProviderKind {
    Codex,
    Openai,
    Deepseek,
    /// Development-only deterministic Provider.
    #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
    Mock,
}

impl From<ProviderKind> for AgentMode {
    fn from(value: ProviderKind) -> Self {
        match value {
            ProviderKind::Codex => Self::Codex,
            ProviderKind::Openai => Self::OpenAI,
            ProviderKind::Deepseek => Self::DeepSeek,
            #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
            ProviderKind::Mock => Self::Mock,
        }
    }
}

#[derive(Args)]
pub(crate) struct ProviderArgs {
    #[arg(long, value_parser = input::id)]
    id: String,
    #[arg(long)]
    name: String,
    #[arg(long, value_enum)]
    kind: ProviderKind,
    #[arg(long, value_parser = input::url)]
    url: Option<reqwest::Url>,
    /// JSON model array `[{"id":"...","name":"...","reasoning_efforts":[]}]`; default empty.
    #[arg(long, value_name = "FILE|-")]
    input: Option<PathBuf>,
    /// Read the secret from redirected stdin, never argv. Removes one trailing LF/CRLF.
    /// Cannot share stdin with --input -. Omit to retain a saved credential.
    #[arg(long)]
    secret_stdin: bool,
}

impl ProviderArgs {
    fn read(
        self,
        stdin: &mut dyn Read,
        source: StdinSource,
    ) -> Result<(AgentProvider, Option<ProviderSecret>), io::Error> {
        if self.secret_stdin && self.input.as_deref() == Some(std::path::Path::new("-")) {
            return Err(input::invalid(
                "agent-provider: --input - and --secret-stdin cannot share stdin",
            ));
        }
        if self.secret_stdin && matches!(source, StdinSource::Terminal) {
            return Err(input::invalid(
                "agent-provider: --secret-stdin requires redirected stdin (terminal echo is unsafe)",
            ));
        }
        let models = self
            .input
            .map(|path| input::json(&path, stdin))
            .transpose()?
            .unwrap_or_default();
        let secret = if self.secret_stdin {
            let mut text = input::read(std::path::Path::new("-"), stdin)?;
            if text.ends_with('\n') {
                text.pop();
                if text.ends_with('\r') {
                    text.pop();
                }
            }
            if text.is_empty() {
                return Err(input::invalid("agent-provider: empty secret stdin"));
            }
            Some(ProviderSecret(text))
        } else {
            None
        };
        Ok((
            AgentProvider {
                id: self.id,
                name: self.name,
                kind: self.kind.into(),
                url: self.url.map(|url| url.to_string()),
                models,
            },
            secret,
        ))
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Scope {
    OneShot,
    Turn,
    Session,
}

impl From<Scope> for ApprovalGrantScope {
    fn from(value: Scope) -> Self {
        match value {
            Scope::OneShot => Self::OneShot,
            Scope::Turn => Self::Turn,
            Scope::Session => Self::Session,
        }
    }
}

#[derive(Args)]
pub(crate) struct ApprovalTarget {
    #[arg(long, value_parser = input::id)]
    run_id: String,
    #[arg(long, value_parser = input::id)]
    approval_id: String,
}

#[derive(Subcommand)]
pub(crate) enum ApprovalCommand {
    /// Approve the recorded request; the daemon validates supported scopes.
    Approve {
        #[command(flatten)]
        target: ApprovalTarget,
        /// Explicit grant scope; inspect the approval kind in run get before choosing.
        #[arg(long, value_enum)]
        scope: Scope,
    },
    /// Deny this operation and allow the turn to continue.
    Deny(ApprovalTarget),
    /// Refuse the request and cancel the native turn.
    Cancel(ApprovalTarget),
}

impl ApprovalCommand {
    fn into_command(self) -> Command {
        let (target, action, scope) = match self {
            Self::Approve { target, scope } => {
                (target, NativeApprovalAction::Approve, Some(scope.into()))
            }
            Self::Deny(target) => (target, NativeApprovalAction::Deny, None),
            Self::Cancel(target) => (target, NativeApprovalAction::Cancel, None),
        };
        Command::ResolveNativeApproval {
            run_id: target.run_id,
            approval_id: target.approval_id,
            action,
            scope,
        }
    }
}

pub(crate) enum Action {
    Execute(Command),
    Export { command: Command, output: PathBuf },
    Events { after: u64 },
}

impl CliCommand {
    pub(crate) fn into_action(
        self,
        stdin: &mut dyn Read,
        source: StdinSource,
    ) -> Result<Action, io::Error> {
        let command = match self {
            Self::Project { command } => return command.into_action(stdin),
            Self::Agent { command } => command.into(),
            Self::AgentProvider { command } => command.into_command(stdin, source)?,
            Self::Session { command } => command.into_command(stdin)?,
            Self::Message { command } => command.into(),
            Self::Run { command } => command.into(),
            Self::Cron { command } => command.into(),
            Self::Settings { command } => command.into_command(stdin)?,
            Self::Event {
                command: EventCommand::List { after },
            }
            | Self::Events { after } => return Ok(Action::Events { after }),
            Self::Export(args) => return Ok(args.into()),
            Self::Import(args) => args.read(stdin)?,
        };
        Ok(Action::Execute(command))
    }
}

impl From<ExportArgs> for Action {
    fn from(args: ExportArgs) -> Self {
        Self::Export {
            command: Command::ExportProject {
                project_id: args.project_id,
            },
            output: args.output,
        }
    }
}

impl ImportArgs {
    fn read(self, stdin: &mut dyn Read) -> Result<Command, io::Error> {
        Ok(Command::ImportProject {
            archive: input::json(&self.input, stdin)?,
            workdir: self.workdir.to_string_lossy().into_owned(),
        })
    }
}

impl ProjectCommand {
    fn into_action(self, stdin: &mut dyn Read) -> Result<Action, io::Error> {
        let command = match self {
            ProjectCommand::List => Command::ListProjects,
            ProjectCommand::Register {
                id,
                name,
                workdir,
                repo_url,
            } => Command::RegisterProject {
                id,
                name,
                workdir: workdir.to_string_lossy().into_owned(),
                repo_url,
            },
            ProjectCommand::SetDefaultAgent {
                project_id,
                agent_id,
            } => Command::SetProjectDefaultAgent {
                project_id,
                agent_id,
            },
            ProjectCommand::Import(args) => args.read(stdin)?,
            ProjectCommand::Export(args) => return Ok(args.into()),
        };
        Ok(Action::Execute(command))
    }
}

impl From<AgentCommand> for Command {
    fn from(value: AgentCommand) -> Self {
        match value {
            AgentCommand::List => Command::ListAgents,
            AgentCommand::Create { id, name, config } => Command::RegisterAgent {
                id,
                name,
                config: config.into(),
            },
            AgentCommand::Update { id, name, config } => Command::UpdateAgent {
                id,
                name,
                config: config.into(),
            },
        }
    }
}

impl AgentProviderCommand {
    fn into_command(self, stdin: &mut dyn Read, source: StdinSource) -> Result<Command, io::Error> {
        let command = match self {
            AgentProviderCommand::List => Command::ListAgentProviders,
            AgentProviderCommand::RefreshModels { provider_id } => {
                Command::RefreshProviderModels { provider_id }
            }
            AgentProviderCommand::Save(args) => {
                let (provider, secret) = args.read(stdin, source)?;
                Command::SaveAgentProvider { provider, secret }
            }
            AgentProviderCommand::DiscoverModels(args) => {
                let (provider, secret) = args.read(stdin, source)?;
                Command::DiscoverProviderModels { provider, secret }
            }
        };
        Ok(command)
    }
}

impl SessionCommand {
    fn into_command(self, stdin: &mut dyn Read) -> Result<Command, io::Error> {
        let command = match self {
            SessionCommand::List { project_id } => Command::ListSessions { project_id },
            SessionCommand::Create {
                id,
                project_id,
                agent_id,
                at_message_id,
            } => Command::CreateSession {
                id,
                project_id,
                agent_id,
                at_message_id,
            },
            SessionCommand::SetAgent {
                session_id,
                agent_id,
            } => Command::SetSessionAgent {
                session_id,
                agent_id,
            },
            SessionCommand::SetConfig { session_id, config } => Command::SetSessionConfig {
                session_id,
                config: config.into(),
            },
            SessionCommand::Rename { session_id, name } => {
                Command::RenameSession { session_id, name }
            }
            SessionCommand::SetTitle { session_id, title } => {
                Command::SetSessionTitle { session_id, title }
            }
            SessionCommand::Send { session_id, text } => Command::SendMessage {
                session_id,
                text: text.read(stdin)?,
            },
            SessionCommand::Fork {
                id,
                project_id,
                agent_id,
                at_message_id,
                text,
            } => Command::ForkSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                text: text.read(stdin)?,
            },
            SessionCommand::Derive {
                id,
                project_id,
                source_session_id,
                agent_id,
                at_message_id,
                text,
            } => Command::DeriveSession {
                id,
                project_id,
                source_session_id,
                agent_id,
                at_message_id,
                text: text.read(stdin)?,
            },
        };
        Ok(command)
    }
}

impl From<MessageCommand> for Command {
    fn from(value: MessageCommand) -> Self {
        match value {
            MessageCommand::List { project_id } => Command::ListMessages { project_id },
        }
    }
}

impl From<RunCommand> for Command {
    fn from(value: RunCommand) -> Self {
        match value {
            RunCommand::List { project_id } => Command::ListRuns { project_id },
            RunCommand::Get { run_id } => Command::GetRun { run_id },
            RunCommand::Cancel { run_id } => Command::CancelRun { run_id },
            RunCommand::Approval { command } => command.into_command(),
        }
    }
}

impl From<CronCommand> for Command {
    fn from(value: CronCommand) -> Self {
        match value {
            CronCommand::List => Command::ListCrons,
            CronCommand::Create {
                id,
                name,
                project_id,
                base_message_id,
                agent_id,
                schedule,
                timezone,
            } => Command::CreateCron {
                id,
                name,
                project_id,
                base_message_id,
                agent_id,
                schedule,
                timezone,
            },
            CronCommand::Enable { cron_id } => Command::SetCronEnabled {
                cron_id,
                enabled: true,
            },
            CronCommand::Disable { cron_id } => Command::SetCronEnabled {
                cron_id,
                enabled: false,
            },
            CronCommand::Trigger {
                cron_id,
                scheduled_at,
            } => Command::TriggerCron {
                cron_id,
                scheduled_at,
            },
        }
    }
}

impl SettingsCommand {
    fn into_command(self, stdin: &mut dyn Read) -> Result<Command, io::Error> {
        let command = match self {
            SettingsCommand::Get => Command::GetSettings,
            SettingsCommand::Reset => Command::ResetSettings,
            SettingsCommand::Set {
                expected_revision,
                input,
            } => Command::SaveSettings {
                expected_revision,
                values: input::json(&input, stdin)?,
            },
        };
        Ok(command)
    }
}
