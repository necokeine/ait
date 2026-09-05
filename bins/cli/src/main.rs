//! AIT command-line client entry point.

use std::{fs, path::PathBuf};

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
    /// Execute any version-one command encoded as JSON.
    Command { json: String },
    /// Print the complete durable workspace projection.
    Snapshot,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let client = reqwest::Client::new();
    match arguments.command {
        CliCommand::Command { json } => {
            let command: Command = serde_json::from_str(&json)?;
            print_response(&send(&client, &arguments.endpoint, &command).await?);
        }
        CliCommand::Snapshot => {
            print_response(&send(&client, &arguments.endpoint, &Command::Snapshot).await?);
        }
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

async fn send(
    client: &reqwest::Client,
    endpoint: &str,
    command: &Command,
) -> Result<Response, reqwest::Error> {
    if matches!(command, Command::Snapshot | Command::GetSettings) {
        return client
            .get(format!("{endpoint}{}", operation_path(command)))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await;
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
        Command::CreateSession { .. } => "/v1/session/create",
        Command::SetSessionAgent { .. } => "/v1/session/set-agent",
        Command::SendMessage { .. } => "/v1/session/send-message",
        Command::ForkSession { .. } => "/v1/session/fork",
        Command::GetRun { .. } => "/v1/run/get",
        Command::CancelRun { .. } => "/v1/run/cancel",
        Command::CreateCron { .. } => "/v1/cron/create",
        Command::SetCronEnabled { .. } => "/v1/cron/set-enabled",
        Command::TriggerCron { .. } => "/v1/cron/trigger",
        Command::ExportProject { .. } => "/v1/project/export",
        Command::ImportProject { .. } => "/v1/project/import",
        Command::GetSettings => "/v1/settings",
        Command::SaveSettings { .. } => "/v1/settings/save",
        Command::ResetSettings => "/v1/settings/reset",
        Command::Snapshot => "/v1/workspace/snapshot",
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
