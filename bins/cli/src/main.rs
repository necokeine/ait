//! AIT command-line client entry point.

use ait_contracts::{Command, Response};
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
                .get(format!("{}/v1/events?after={after}", arguments.endpoint))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            print!("{body}");
        }
    }
    Ok(())
}

async fn send(
    client: &reqwest::Client,
    endpoint: &str,
    command: &Command,
) -> Result<Response, reqwest::Error> {
    client
        .post(format!("{endpoint}/v1/commands"))
        .json(command)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
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
