//! Immutable conversation messages and interactive Run creation.
use crate::control::catalog::{require_agent, validate_config};
use crate::control::errors::error;
use crate::control::events::{now, pending};
use crate::control::execution::CommandOutcome;
use crate::control::permissions::{PermissionPolicyLimits, effective_permission_profile};
use crate::control::project::git::{GitBaseline, is_git_commit};
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, MessageView, ProjectView, RunView};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use serde_json::Value;
use std::collections::HashSet;
use std::fmt::Write as _;
use uuid::Uuid;

pub(in crate::control) fn send_message(
    state: &mut WorkingSet,
    session_id: String,
    text: String,
    git_baseline: &GitBaseline,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if text.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidMessageRole,
            "message text is required",
            false,
        ));
    }
    let index = state
        .sessions
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let session = state.sessions[index].clone();
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    let agent = require_agent(state, &session.agent_id)?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    let permission_profile =
        effective_permission_profile(&state.settings, &provider, permission_limits)?;
    let user = message(
        &session.project_id,
        Some(&session.current_message_id),
        "user",
        "standard",
        Some(text),
        Some(&git_baseline.commit),
        None,
    );
    state.messages.push(user.clone());
    let run_id = Uuid::new_v4().to_string();
    state.sessions[index]
        .current_message_id
        .clone_from(&user.id);
    state.sessions[index].version += 1;
    state.sessions[index].active_run_id = Some(run_id.clone());
    let workspace_base_commit = Some(git_baseline.commit.clone());
    let workspace_base_index_tree = Some(git_baseline.index_tree.clone().into_boxed_str());
    let run = RunView {
        execution: None,
        id: run_id.clone(),
        project_id: session.project_id,
        base_message_id: user.id,
        last_message_id: None,
        session_id: Some(session_id),
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        permission_profile,
        native_approvals: Vec::new(),
        trigger: "manual".into(),
        cron_id: None,
        scheduled_at: None,
        workspace_base_commit,
        workspace_base_index_tree,
        status: "queued".into(),
        phase: Some("queued".into()),
        operation_id: Some(format!("workspace-{run_id}").into_boxed_str()),
        lease_epoch: 0,
        error: None,
    };
    if let Some(reference) = state.provider_credentials.get(&agent.config.provider_id) {
        state
            .run_credentials
            .insert(run_id.clone(), reference.clone());
    }
    state.runs.push(run);
    let run = state.runs.last().expect("new run exists").clone();
    let event = pending("run.updated", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
}

pub(in crate::control) fn codex_prompt(
    state: &WorkingSet,
    head_id: &str,
) -> Result<(Option<String>, String), ApiError> {
    let mut path = Vec::new();
    let mut current = Some(head_id);
    while let Some(id) = current {
        let message = state
            .messages
            .iter()
            .find(|message| message.id == id)
            .ok_or_else(|| {
                error(
                    ErrorCode::MessageNotFound,
                    "message path is incomplete",
                    false,
                )
            })?;
        path.push(message);
        current = message.parent_message_id.as_deref();
    }
    path.reverse();
    let mut instructions = Vec::new();
    let mut prompt = String::from("Conversation:\n");
    for message in path {
        if let Some(text) = &message.text {
            if message.role == "system" {
                instructions.push(text.as_str());
            } else {
                let _ = writeln!(prompt, "{}: {text}", message.role);
            }
        }
    }
    Ok((
        (!instructions.is_empty()).then(|| instructions.join("\n\n")),
        prompt,
    ))
}

pub(in crate::control) fn append_output(
    state: &mut WorkingSet,
    run: &mut RunView,
    output: MessageView,
) {
    run.last_message_id = Some(output.id.clone());
    if let Some(session_id) = &run.session_id
        && let Some(session) = state
            .sessions
            .iter_mut()
            .find(|session| &session.id == session_id)
    {
        session.current_message_id.clone_from(&output.id);
        session.version += 1;
    }
    state.messages.push(output);
}

pub(in crate::control) fn message(
    project: &str,
    parent: Option<&str>,
    role: &str,
    kind: &str,
    text: Option<String>,
    git_commit: Option<&str>,
    data: Option<Value>,
) -> MessageView {
    MessageView {
        id: Uuid::new_v4().to_string(),
        project_id: project.into(),
        parent_message_id: parent.map(str::to_owned),
        role: role.into(),
        kind: kind.into(),
        text,
        git_commit: git_commit.map(str::to_owned),
        data,
        created_at: now(),
    }
}

pub(in crate::control) fn validate_message_text(text: &str) -> Result<(), ApiError> {
    if text.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidMessageRole,
            "message text is required",
            false,
        ));
    }
    Ok(())
}

pub(in crate::control) fn validate_session_message(
    state: &WorkingSet,
    project_id: &str,
    message_id: &str,
) -> Result<(), ApiError> {
    let message = state
        .messages
        .iter()
        .find(|message| message.id == message_id)
        .ok_or_else(|| {
            error(
                ErrorCode::MessageNotFound,
                "branch message not found",
                false,
            )
        })?;
    if message.project_id != project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "branch message belongs to another project",
            false,
        ));
    }
    Ok(())
}

pub(in crate::control) fn message_workspace_commit(
    state: &WorkingSet,
    project: &ProjectView,
    message_id: &str,
) -> Result<String, ApiError> {
    let mut cursor = Some(message_id);
    let mut seen = HashSet::new();
    while let Some(id) = cursor {
        if !seen.insert(id) {
            return Err(error(
                ErrorCode::InvalidMessageId,
                "message path contains a cycle",
                false,
            ));
        }
        let message = state
            .messages
            .iter()
            .find(|message| message.id == id)
            .ok_or_else(|| {
                error(
                    ErrorCode::MessageNotFound,
                    "message path is incomplete",
                    false,
                )
            })?;
        if message.project_id != project.id {
            return Err(error(
                ErrorCode::SessionMessageProjectMismatch,
                "message path belongs to another project",
                false,
            ));
        }
        if let Some(commit) = message
            .data
            .as_ref()
            .and_then(|data| data.get("codex"))
            .and_then(|codex| codex.get("commit_id"))
            .and_then(Value::as_str)
            .filter(|commit| is_git_commit(commit))
        {
            return Ok(commit.to_owned());
        }
        if let Some(commit) = message.git_commit.as_deref() {
            return Ok(commit.to_owned());
        }
        cursor = message.parent_message_id.as_deref();
    }
    Ok(project.base_commit.clone())
}
