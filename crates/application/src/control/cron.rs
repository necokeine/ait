//! Cron configuration, enablement and idempotent Run triggers.
use crate::control::catalog::{require_agent, require_named_agent, validate_config};
use crate::control::errors::error;
use crate::control::events::{now, pending};
use crate::control::execution::CommandOutcome;
use crate::control::permissions::{PermissionPolicyLimits, effective_permission_profile};
use crate::control::project::git::GitBaseline;
use crate::control::state::WorkingSet;
use ait_contracts::{AgentMode, ApiError, CommandResult, CronView, RunView};
use ait_domain::{
    AgentId, Cron, CronConcurrencyPolicy, CronId, CronMisfirePolicy, ErrorCode, MessageId,
    ProjectId, TimestampMs,
};
use ait_ports::PendingEvent;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub(in crate::control) fn create_cron(
    state: &mut WorkingSet,
    id: String,
    name: String,
    project_id: String,
    base_message_id: String,
    agent_id: String,
    schedule: String,
    timezone: String,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if state.crons.iter().any(|cron| cron.id == id) {
        return Err(error(
            ErrorCode::InvalidCron,
            "cron id already exists",
            false,
        ));
    }
    let base = state
        .messages
        .iter()
        .find(|message| message.id == base_message_id)
        .ok_or_else(|| {
            error(
                ErrorCode::CronBaseMessageUnavailable,
                "cron base message not found",
                false,
            )
        })?;
    if base.project_id != project_id {
        return Err(error(
            ErrorCode::CronBaseMessageUnavailable,
            "cron base message belongs to another project",
            false,
        ));
    }
    require_named_agent(state, &agent_id).map_err(|_| {
        error(
            ErrorCode::CronAgentUnavailable,
            "cron agent unavailable",
            false,
        )
    })?;
    let domain = Cron {
        id: CronId::new(&id),
        name: name.clone(),
        project_id: ProjectId::new(&project_id),
        base_message_id: MessageId::parse(&base_message_id)
            .map_err(|_| error(ErrorCode::InvalidCron, "invalid base message id", false))?,
        agent_id: AgentId::new(&agent_id),
        schedule: schedule.clone(),
        timezone: timezone.clone(),
        enabled: true,
        concurrency_policy: CronConcurrencyPolicy::Forbid,
        misfire_policy: CronMisfirePolicy::RunOnce,
        max_runtime: None,
        next_run_at: ait_scheduler::next_occurrence(&schedule, &timezone, TimestampMs(now()))
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?,
        last_run_at: None,
        version: 1,
        created_at: TimestampMs(now()),
        updated_at: TimestampMs(now()),
    };
    domain
        .validate()
        .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
    let cron = CronView {
        id: id.clone(),
        name,
        project_id,
        base_message_id,
        agent_id,
        schedule,
        timezone,
        enabled: true,
    };
    state.crons.push(cron.clone());
    Ok((
        CommandResult::Cron(cron.clone()),
        vec![pending("cron.created", Some(id), &cron)],
    ))
}

pub(in crate::control) fn set_cron_enabled(
    state: &mut WorkingSet,
    cron_id: &str,
    enabled: bool,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let cron = state
        .crons
        .iter_mut()
        .find(|cron| cron.id == cron_id)
        .ok_or_else(|| error(ErrorCode::InvalidCron, "cron not found", false))?;
    cron.enabled = enabled;
    let cron = cron.clone();
    Ok((
        CommandResult::Cron(cron.clone()),
        vec![pending(
            "cron.enabled_changed",
            Some(cron.id.clone()),
            &cron,
        )],
    ))
}

pub(in crate::control) fn trigger_cron(
    state: &mut WorkingSet,
    cron_id: &str,
    scheduled_at: i64,
    workspace_baseline: Option<&GitBaseline>,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if let Some(existing) = state.runs.iter().find(|run| {
        run.cron_id.as_deref() == Some(cron_id) && run.scheduled_at == Some(scheduled_at)
    }) {
        return Ok((
            CommandOutcome::Ready(Box::new(CommandResult::Run(existing.clone()))),
            Vec::new(),
        ));
    }
    let cron = state
        .crons
        .iter()
        .find(|cron| cron.id == cron_id && cron.enabled)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidCron, "enabled cron not found", false))?;
    let agent = require_agent(state, &cron.agent_id)?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    let permission_profile =
        effective_permission_profile(&state.settings, &provider, permission_limits)?;
    if provider.kind == AgentMode::Codex && workspace_baseline.is_none() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Codex Cron Run requires a Git baseline captured under the workspace lease",
            true,
        ));
    }
    let run_id = Uuid::new_v4().to_string();
    if let Some(reference) = state.provider_credentials.get(&agent.config.provider_id) {
        state
            .run_credentials
            .insert(run_id.clone(), reference.clone());
    }
    state.runs.push(RunView {
        execution: None,
        id: run_id.clone(),
        project_id: cron.project_id,
        base_message_id: cron.base_message_id,
        last_message_id: None,
        session_id: None,
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        permission_profile,
        native_approvals: Vec::new(),
        trigger: "cron".into(),
        cron_id: Some(cron.id),
        scheduled_at: Some(scheduled_at),
        workspace_base_commit: workspace_baseline.map(|baseline| baseline.commit.clone()),
        workspace_base_index_tree: workspace_baseline
            .map(|baseline| baseline.index_tree.clone().into_boxed_str()),
        status: "queued".into(),
        phase: Some("queued".into()),
        operation_id: Some(format!("workspace-{run_id}").into_boxed_str()),
        lease_epoch: 0,
        error: None,
    });
    let run = state.runs.last().expect("new run exists").clone();
    let event = pending("cron.run_triggered", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
}
