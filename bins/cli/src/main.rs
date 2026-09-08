//! AIT command-line client entry point.

use std::{fs, io, path::PathBuf};

use ait_contracts::{Command, CommandResult, ProjectExport, Response};
use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Arguments {
    /// Local daemon HTTP endpoint.
    #[arg(long, default_value = "http://127.0.0.1:7314")]
    endpoint: String,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Execute low-level JSON, or use `-` to read it from stdin.
    Command { json: String },
    /// Inspect Projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Inspect Agents.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Inspect Agent Providers.
    AgentProvider {
        #[command(subcommand)]
        command: AgentProviderCommand,
    },
    /// Inspect Sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Inspect Messages.
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Inspect Runs.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Inspect Crons.
    Cron {
        #[command(subcommand)]
        command: CronCommand,
    },
    /// Replay durable Server-Sent Events after a cursor.
    Events {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    /// Export one Project and all of its Message branches and Session refs.
    Export {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Import a portable Project archive into an explicit local workdir.
    Import {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        workdir: PathBuf,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// List Projects.
    List,
}

#[derive(Subcommand)]
enum AgentCommand {
    /// List Agents.
    List,
}

#[derive(Subcommand)]
enum AgentProviderCommand {
    /// List Agent Providers.
    List,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// List Sessions, optionally scoped to one Project.
    List {
        #[arg(long)]
        project_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum MessageCommand {
    /// List Messages in one Project.
    List {
        #[arg(long)]
        project_id: String,
    },
}

#[derive(Subcommand)]
enum RunCommand {
    /// List Runs in one Project.
    List {
        #[arg(long)]
        project_id: String,
    },
}

#[derive(Subcommand)]
enum CronCommand {
    /// List Crons.
    List,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let client = reqwest::Client::new();
    match arguments.command {
        CliCommand::Command { json } => {
            execute(&client, &arguments.endpoint, parse_command(json)?).await?;
        }
        CliCommand::Project {
            command: ProjectCommand::List,
        } => execute(&client, &arguments.endpoint, Command::ListProjects).await?,
        CliCommand::Agent {
            command: AgentCommand::List,
        } => execute(&client, &arguments.endpoint, Command::ListAgents).await?,
        CliCommand::AgentProvider {
            command: AgentProviderCommand::List,
        } => execute(&client, &arguments.endpoint, Command::ListAgentProviders).await?,
        CliCommand::Session {
            command: SessionCommand::List { project_id },
        } => {
            execute(
                &client,
                &arguments.endpoint,
                Command::ListSessions { project_id },
            )
            .await?;
        }
        CliCommand::Message {
            command: MessageCommand::List { project_id },
        } => {
            execute(
                &client,
                &arguments.endpoint,
                Command::ListMessages { project_id },
            )
            .await?;
        }
        CliCommand::Run {
            command: RunCommand::List { project_id },
        } => {
            execute(
                &client,
                &arguments.endpoint,
                Command::ListRuns { project_id },
            )
            .await?;
        }
        CliCommand::Cron {
            command: CronCommand::List,
        } => execute(&client, &arguments.endpoint, Command::ListCrons).await?,
        CliCommand::Events { after } => {
            let body = client
                .get(format!(
                    "{}/v1/event/list?after={after}",
                    arguments.endpoint
                ))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            print!("{body}");
        }
        CliCommand::Export { project_id, output } => {
            let response = send(
                &client,
                &arguments.endpoint,
                &Command::ExportProject { project_id },
            )
            .await?;
            match &response.result {
                Some(CommandResult::ProjectExport(archive)) if response.ok => {
                    fs::write(output, serde_json::to_vec_pretty(&archive)?)?;
                }
                _ => print_response(&response),
            }
        }
        CliCommand::Import { input, workdir } => {
            let archive: ProjectExport = serde_json::from_slice(&fs::read(input)?)?;
            let response = send(
                &client,
                &arguments.endpoint,
                &Command::ImportProject {
                    archive,
                    workdir: workdir.to_string_lossy().into_owned(),
                },
            )
            .await?;
            print_response(&response);
        }
    }
    Ok(())
}

fn parse_command(json: String) -> Result<Command, io::Error> {
    let input = if json == "-" {
        io::read_to_string(io::stdin())?
    } else {
        json
    };
    // Deserialization errors can quote unknown variants/fields, including a secret.
    serde_json::from_str(&input).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid command JSON at line {}, column {}",
                error.line(),
                error.column()
            ),
        )
    })
}

async fn execute(
    client: &reqwest::Client,
    endpoint: &str,
    command: Command,
) -> Result<(), reqwest::Error> {
    print_response(&send(client, endpoint, &command).await?);
    Ok(())
}

async fn send(
    client: &reqwest::Client,
    endpoint: &str,
    command: &Command,
) -> Result<Response, reqwest::Error> {
    if matches!(
        command,
        Command::GetSettings
            | Command::ListProjects
            | Command::ListAgents
            | Command::ListAgentProviders
            | Command::ListSessions { .. }
            | Command::ListMessages { .. }
            | Command::ListRuns { .. }
            | Command::ListCrons
    ) {
        let request = client.get(format!("{endpoint}{}", operation_path(command)));
        let request = match command {
            Command::ListSessions {
                project_id: Some(project_id),
            }
            | Command::ListMessages { project_id }
            | Command::ListRuns { project_id } => request.query(&[("project_id", project_id)]),
            _ => request,
        };
        return request.send().await?.error_for_status()?.json().await;
    }

    let mut body = serde_json::to_value(command).expect("command serializes");
    body.as_object_mut()
        .expect("command serializes as an object")
        .remove("type");
    client
        .post(format!("{endpoint}{}", operation_path(command)))
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

const fn operation_path(command: &Command) -> &'static str {
    match command {
        Command::RegisterProject { .. } => "/v1/project/register",
        Command::SetProjectDefaultAgent { .. } => "/v1/project/set-default-agent",
        Command::RegisterAgent { .. } => "/v1/agent/register",
        Command::UpdateAgent { .. } => "/v1/agent/update",
        Command::SaveAgentProvider { .. } => "/v1/agent-provider/save",
        Command::DiscoverProviderModels { .. } => "/v1/agent-provider/discover-models",
        Command::RefreshProviderModels { .. } => "/v1/agent-provider/refresh-models",
        Command::SetSessionConfig { .. } => "/v1/session/set-config",
        Command::CreateSession { .. } => "/v1/session/create",
        Command::SetSessionAgent { .. } => "/v1/session/set-agent",
        Command::RenameSession { .. } => "/v1/session/rename",
        Command::SetSessionTitle { .. } => "/v1/session/set-title",
        Command::SendMessage { .. } => "/v1/session/send-message",
        Command::ForkSession { .. } => "/v1/session/fork",
        Command::GetRun { .. } => "/v1/run/get",
        Command::CancelRun { .. } => "/v1/run/cancel",
        Command::ResolveNativeApproval { .. } => "/v1/run/approval/resolve",
        Command::CreateCron { .. } => "/v1/cron/create",
        Command::SetCronEnabled { .. } => "/v1/cron/set-enabled",
        Command::TriggerCron { .. } => "/v1/cron/trigger",
        Command::ExportProject { .. } => "/v1/project/export",
        Command::ImportProject { .. } => "/v1/project/import",
        Command::GetSettings => "/v1/settings",
        Command::SaveSettings { .. } => "/v1/settings/save",
        Command::ResetSettings => "/v1/settings/reset",
        Command::ListProjects => "/v1/project/list",
        Command::ListAgents => "/v1/agent/list",
        Command::ListAgentProviders => "/v1/agent-provider/list",
        Command::ListSessions { .. } => "/v1/session/list",
        Command::ListMessages { .. } => "/v1/message/list",
        Command::ListRuns { .. } => "/v1/run/list",
        Command::ListCrons => "/v1/cron/list",
    }
}

fn print_response(response: &Response) {
    println!(
        "{}",
        serde_json::to_string_pretty(&response).expect("response serializes")
    );
    if !response.ok {
        std::process::exit(2);
    }
}
