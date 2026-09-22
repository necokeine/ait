use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use server_application::workspace_automation::{
    ScriptSnapshot, ScriptType, SetupLifecycle, SetupSnapshot, SetupStatus, WorkspaceAutomation,
};
use server_domain::registry::UntrustedWorkspaceSource;
use server_protocol::ErrorCode;
use server_protocol::workspace_automation::{
    WorkspaceBlockedSource, WorkspaceScript, WorkspaceScriptLifecycle, WorkspaceScriptListResult,
    WorkspaceScriptMutationResult, WorkspaceScriptRequest, WorkspaceScriptType,
    WorkspaceSetupCommand, WorkspaceSetupCommandStatus, WorkspaceSetupDetail,
    WorkspaceSetupRequest, WorkspaceSetupRunResult, WorkspaceSetupSnapshot, WorkspaceSetupStatus,
    WorkspaceSetupStatusResult,
};

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.workspace_automation.clone(),
        ErrorCode::RegistryIo,
        move |automation| execute(automation, &method, params),
    )
    .await
}

fn execute(
    automation: &WorkspaceAutomation,
    method: &str,
    params: Value,
) -> Result<Value, ErrorCode> {
    match method {
        "workspace.setup.status.request" => setup_status(automation, decode(params)?),
        "workspace.setup.run.request" => setup_run(automation, decode(params)?),
        "workspace.script.list.request" => list_scripts(automation, decode(params)?),
        "workspace.script.start.request" => start_script(automation, &decode(params)?),
        "workspace.script.stop.request" => stop_script(automation, &decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn setup_status(
    automation: &WorkspaceAutomation,
    request: WorkspaceSetupRequest,
) -> Result<Value, ErrorCode> {
    let snapshot = match automation.setup_status(&request.workspace_id) {
        Ok(SetupStatus::Absent) => None,
        Ok(SetupStatus::Snapshot(snapshot)) => Some(setup_snapshot(snapshot, None)),
        Ok(SetupStatus::Blocked { placement, source }) => Some(WorkspaceSetupSnapshot {
            status: WorkspaceSetupStatus::Blocked,
            detail: WorkspaceSetupDetail {
                kind: "worktree_setup".to_owned(),
                worktree_path: placement.worktree_path,
                branch_name: placement.branch_name,
                log: String::new(),
                commands: Vec::new(),
                truncated: false,
            },
            error: None,
            blocked_source: Some(blocked_source(source)),
        }),
        Err(error) => {
            return encode(WorkspaceSetupStatusResult {
                workspace_id: request.workspace_id,
                snapshot: Some(WorkspaceSetupSnapshot {
                    status: WorkspaceSetupStatus::Failed,
                    detail: WorkspaceSetupDetail {
                        kind: "worktree_setup".to_owned(),
                        worktree_path: String::new(),
                        branch_name: String::new(),
                        log: String::new(),
                        commands: Vec::new(),
                        truncated: false,
                    },
                    error: Some(error.to_string()),
                    blocked_source: None,
                }),
            });
        }
    };
    encode(WorkspaceSetupStatusResult {
        workspace_id: request.workspace_id,
        snapshot,
    })
}

fn setup_run(
    automation: &WorkspaceAutomation,
    request: WorkspaceSetupRequest,
) -> Result<Value, ErrorCode> {
    let result = automation.approve_and_start_setup(&request.workspace_id, &timestamp());
    let (started, error) = match result {
        Ok(started) => (started, None),
        Err(error) => (false, Some(error.to_string())),
    };
    encode(WorkspaceSetupRunResult {
        workspace_id: request.workspace_id,
        started,
        error,
    })
}

fn list_scripts(
    automation: &WorkspaceAutomation,
    request: WorkspaceSetupRequest,
) -> Result<Value, ErrorCode> {
    let result = automation.list_scripts(&request.workspace_id);
    let (scripts, error) = match result {
        Ok(scripts) => (scripts.into_iter().map(script).collect(), None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    encode(WorkspaceScriptListResult {
        workspace_id: request.workspace_id,
        scripts,
        error,
    })
}

fn start_script(
    automation: &WorkspaceAutomation,
    request: &WorkspaceScriptRequest,
) -> Result<Value, ErrorCode> {
    script_mutation(
        request,
        automation.start_script(&request.workspace_id, &request.script_name),
    )
}

fn stop_script(
    automation: &WorkspaceAutomation,
    request: &WorkspaceScriptRequest,
) -> Result<Value, ErrorCode> {
    script_mutation(
        request,
        automation.stop_script(&request.workspace_id, &request.script_name),
    )
}

fn script_mutation(
    request: &WorkspaceScriptRequest,
    result: Result<
        ScriptSnapshot,
        server_application::workspace_automation::WorkspaceAutomationServiceError,
    >,
) -> Result<Value, ErrorCode> {
    let (script, error) = match result {
        Ok(snapshot) => (Some(script(snapshot)), None),
        Err(error) => (None, Some(error.to_string())),
    };
    encode(WorkspaceScriptMutationResult {
        workspace_id: request.workspace_id.clone(),
        script_name: request.script_name.clone(),
        script,
        error,
    })
}

fn setup_snapshot(
    snapshot: SetupSnapshot,
    blocked_source: Option<WorkspaceBlockedSource>,
) -> WorkspaceSetupSnapshot {
    WorkspaceSetupSnapshot {
        status: match snapshot.lifecycle {
            SetupLifecycle::Running => WorkspaceSetupStatus::Running,
            SetupLifecycle::Completed => WorkspaceSetupStatus::Completed,
            SetupLifecycle::Failed => WorkspaceSetupStatus::Failed,
        },
        detail: WorkspaceSetupDetail {
            kind: "worktree_setup".to_owned(),
            worktree_path: snapshot.worktree_path,
            branch_name: snapshot.branch_name,
            log: snapshot.log,
            commands: snapshot
                .commands
                .into_iter()
                .map(|command| WorkspaceSetupCommand {
                    index: command.index,
                    command: command.command,
                    cwd: command.cwd,
                    log: command.log,
                    status: if command.running {
                        WorkspaceSetupCommandStatus::Running
                    } else if command.exit_code == Some(0) {
                        WorkspaceSetupCommandStatus::Completed
                    } else {
                        WorkspaceSetupCommandStatus::Failed
                    },
                    exit_code: command.exit_code,
                    duration_ms: command.duration_ms,
                })
                .collect(),
            truncated: snapshot.truncated,
        },
        error: snapshot.error,
        blocked_source,
    }
}

fn blocked_source(source: UntrustedWorkspaceSource) -> WorkspaceBlockedSource {
    match source {
        UntrustedWorkspaceSource::ChangeRequest {
            forge,
            number,
            head_repository,
        } => WorkspaceBlockedSource::ChangeRequest {
            forge,
            number,
            head_repository,
        },
    }
}

fn script(snapshot: ScriptSnapshot) -> WorkspaceScript {
    WorkspaceScript {
        script_name: snapshot.name,
        kind: match snapshot.kind {
            ScriptType::Script => WorkspaceScriptType::Script,
            ScriptType::Service => WorkspaceScriptType::Service,
        },
        hostname: snapshot.hostname,
        port: snapshot.port,
        local_proxy_url: None,
        public_proxy_url: None,
        proxy_url: None,
        lifecycle: if snapshot.running {
            WorkspaceScriptLifecycle::Running
        } else {
            WorkspaceScriptLifecycle::Stopped
        },
        health: None,
        exit_code: snapshot.exit_code,
        terminal_id: snapshot.terminal_id,
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::RegistryIo)
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests;
