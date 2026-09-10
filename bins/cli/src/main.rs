//! AIT command-line client entry point.

mod args;
mod input;

use ait_contracts::{Command, CommandResult, Response};
use args::{Action, Arguments};
use clap::Parser;
use std::{
    fs,
    io::{self, IsTerminal},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let stdin = io::stdin();
    let source = if stdin.is_terminal() {
        input::StdinSource::Terminal
    } else {
        input::StdinSource::Redirected
    };
    let action = arguments.command.into_action(&mut stdin.lock(), source)?;
    let endpoint = arguments.endpoint.as_str().trim_end_matches('/');
    let client = reqwest::Client::new();
    match action {
        Action::Execute(command) => {
            let response = send(&client, endpoint, &command)
                .await
                .map_err(|error| request_error(error, &command))?;
            print_response(&response, &command);
        }
        Action::Events { after } => {
            let body = client
                .get(format!("{endpoint}/v1/event/list?after={after}"))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            print!("{body}");
        }
        Action::Export { command, output } => {
            let response = send(&client, endpoint, &command).await?;
            match &response.result {
                Some(CommandResult::ProjectExport(archive)) if response.ok => {
                    fs::write(output, serde_json::to_vec_pretty(archive)?)?;
                }
                _ => print_response(&response, &command),
            }
        }
    }
    Ok(())
}

fn request_error(error: reqwest::Error, command: &Command) -> Box<dyn std::error::Error> {
    if matches!(
        command,
        Command::SaveAgentProvider {
            secret: Some(_),
            ..
        } | Command::DiscoverProviderModels {
            secret: Some(_),
            ..
        }
    ) {
        // A malformed server response can make serde quote an unknown variant
        // containing the secret. Never render the underlying error on this path.
        input::invalid("agent-provider request failed (transport, HTTP status or response decoding); check the connection and saved Provider state").into()
    } else {
        error.into()
    }
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
            Command::ListSessions { project_id }
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
        Command::DeriveSession { .. } => "/v1/session/derive",
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

fn print_response(response: &Response, command: &Command) {
    println!("{}", response_output(response, command));
    if !response.ok {
        std::process::exit(2);
    }
}

fn response_output(response: &Response, command: &Command) -> String {
    // Defense in depth if a remote error ever echoes a credential. Redact decoded
    // strings so escaping cannot evade it and JSON syntax is always preserved.
    if let Command::SaveAgentProvider {
        secret: Some(secret),
        ..
    }
    | Command::DiscoverProviderModels {
        secret: Some(secret),
        ..
    } = command
    {
        let mut output = serde_json::to_value(response).expect("response serializes");
        redact(&mut output, &secret.0);
        return serde_json::to_string_pretty(&output).expect("response serializes");
    }
    serde_json::to_string_pretty(response).expect("response serializes")
}

fn redact(value: &mut serde_json::Value, secret: &str) {
    if secret.is_empty() {
        return;
    }
    match value {
        serde_json::Value::String(text) => *text = text.replace(secret, "[REDACTED]"),
        serde_json::Value::Array(values) => {
            for value in values {
                redact(value, secret);
            }
        }
        serde_json::Value::Object(values) => {
            // The response's schema keys are public protocol, not secret data.
            for value in values.values_mut() {
                redact(value, secret);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
