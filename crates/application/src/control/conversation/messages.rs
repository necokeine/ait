//! Immutable conversation messages and interactive Run creation.
use crate::control::conversation::MessageRecord;
use crate::control::project::ProjectRecord;
use crate::control::runs::{RunLifecycle, RunRecord};

use crate::control::catalog::{require_agent, validate_config};
use crate::control::errors::error;
use crate::control::events::{now, pending};
use crate::control::execution::CommandOutcome;
use crate::control::permissions::{PermissionPolicyLimits, effective_permission_profile};
use crate::control::persistence::{
    HasAgents, HasMessages, HasProviderCredentials, HasProviders, HasRunCredentials, HasRuns,
    HasSessions, HasSettings,
};
use crate::control::project::git::{GitBaseline, is_git_commit};
use ait_contracts::ApiError;
use ait_domain::{ErrorCode, SessionSource};
use ait_ports::PendingEvent;
use serde_json::Value;
use std::collections::HashSet;
use std::fmt::Write as _;
use uuid::Uuid;

#[allow(
    clippy::too_many_lines,
    reason = "Run creation and its first immutable Message must remain one reducer transition"
)]
pub(in crate::control) fn send_message(
    state: &mut (
             impl HasAgents
             + HasMessages
             + HasProviderCredentials
             + HasProviders
             + HasRunCredentials
             + HasRuns
             + HasSessions
             + HasSettings
         ),
    session_id: String,
    text: String,
    git_baseline: Option<&GitBaseline>,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    validate_message_text(&text)?;
    let index = state
        .sessions()
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let session = state.sessions()[index].clone();
    if session.active_run_id().is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    let agent = require_agent(state, session.agent_id())?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    let permission_profile =
        effective_permission_profile(state.settings(), &provider, permission_limits)?;
    if matches!(session.source, SessionSource::CodexThread(_)) {
        return Err(error(
            ErrorCode::CodexThreadCapabilityUnsupported,
            "native Codex input requires exclusive writer admission",
            false,
        ));
    }
    let git_baseline = git_baseline.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git HEAD/index snapshot is missing for user message",
            false,
        )
    })?;
    let run_id = Uuid::new_v4().to_string();
    let user = message(
        &session.project_id,
        Some(&session.current_message_id()),
        ait_domain::MessageRole::User,
        ait_domain::MessageKind::Standard,
        Some(text),
        Some(&git_baseline.commit),
        None,
    );
    state.messages_mut().push(user.clone());
    let reference = &mut state.sessions_mut()[index].reference;
    reference
        .advance(
            reference.head(),
            reference.version(),
            Some(reference.head()),
            ait_domain::MessageId::parse(&user.id).map_err(|_| {
                error(
                    ErrorCode::InvalidMessageId,
                    "invalid Message identity",
                    false,
                )
            })?,
        )
        .map_err(|e| error(e.code, e.message, e.retryable))?;
    reference
        .acquire(ait_domain::RunId::new(&run_id))
        .map_err(|e| error(e.code, e.message, e.retryable))?;
    let workspace_base_commit = Some(git_baseline.commit.clone());
    let workspace_base_index_tree = Some(git_baseline.index_tree.clone().into_boxed_str());
    let run = RunRecord {
        compatibility_repair: false,
        codex_input: None,
        lifecycle: RunLifecycle::queued(),
        id: run_id.clone(),
        project_id: session.project_id,
        base_message_id: user.id,
        session_id: Some(session_id),
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        permission_profile,
        native_approvals: Vec::new(),
        tool_approvals: Vec::new(),
        tool_interactions: Vec::new(),
        trigger: ait_domain::RunTrigger::Manual,
        cron_id: None,
        scheduled_at: None,
        workspace_base_commit,
        workspace_base_index_tree,
        operation_id: Some(format!("workspace-{run_id}").into_boxed_str()),
        lease_epoch: 0,
    };
    if let Some(reference) = state
        .provider_credentials()
        .get(&agent.config.provider_id)
        .cloned()
    {
        state
            .run_credentials_mut()
            .insert(run_id.clone(), reference.clone());
    }
    state.runs_mut().push(run);
    let run = state.runs().last().expect("new run exists").clone();
    let event = pending("run.updated", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
}

pub(in crate::control) fn codex_prompt(
    state: &impl HasMessages,
    head_id: &str,
) -> Result<(Option<String>, String), ApiError> {
    let path = crate::control::conversation::domain_path(state.messages(), head_id)
        .map_err(|e| error(e.code, e.message, e.retryable))?;
    let mut instructions = Vec::new();
    let mut prompt = String::from("Conversation:\n");
    for message in &path {
        for part in &message.sub_messages {
            if let ait_domain::SubMessage::Text { text } = part {
                if message.role == ait_domain::MessageRole::System {
                    instructions.push(text.as_str());
                } else {
                    let _ = writeln!(prompt, "{}: {text}", message.role.as_str());
                }
            }
        }
    }
    Ok((
        (!instructions.is_empty()).then(|| instructions.join("\n\n")),
        prompt,
    ))
}

pub(in crate::control) fn append_output(
    state: &mut (impl HasMessages + HasSessions),
    run: &mut RunRecord,
    output: MessageRecord,
) {
    run.set_last_message_id(Some(output.id.clone()));
    if let Some(session_id) = &run.session_id
        && let Some(session) = state
            .sessions_mut()
            .iter_mut()
            .find(|session| &session.id == session_id)
    {
        session
            .reference
            .advance(
                session.reference.head(),
                session.reference.version(),
                output
                    .parent_message_id
                    .as_deref()
                    .map(ait_domain::MessageId::parse)
                    .transpose()
                    .expect("validated Message parent"),
                ait_domain::MessageId::parse(&output.id).expect("generated UUID"),
            )
            .expect("settlement holds the Session pointer and finalization gate");
    }
    state.messages_mut().push(output);
}

pub(in crate::control) fn message(
    project: &str,
    parent: Option<&str>,
    role: ait_domain::MessageRole,
    kind: ait_domain::MessageKind,
    text: Option<String>,
    git_commit: Option<&str>,
    data: Option<Value>,
) -> MessageRecord {
    MessageRecord {
        id: Uuid::new_v4().to_string(),
        project_id: project.into(),
        parent_message_id: parent.map(str::to_owned),
        role,
        kind,
        text,
        git_commit: git_commit.map(str::to_owned),
        data,
        created_at: now(),
    }
}

pub(in crate::control) fn validate_message_text(text: &str) -> Result<(), ApiError> {
    ait_domain::message::validate_message_text(text)
        .map_err(|e| error(e.code, e.message, e.retryable))
}

pub(in crate::control) fn validate_session_message(
    state: &impl HasMessages,
    project_id: &str,
    message_id: &str,
) -> Result<(), ApiError> {
    let message = state
        .messages()
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
    state: &impl HasMessages,
    project: &ProjectRecord,
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
            .messages()
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
