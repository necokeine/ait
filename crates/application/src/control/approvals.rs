//! Durable native approval requests, decisions, grants and waiter notification.
use crate::control::LocalControlService;
use crate::control::errors::{api_domain_error, error, store_error};
use crate::control::events::{now, pending};
use crate::control::permissions::{PermissionPolicyLimits, validate_native_permission_profile};
use crate::control::runs::{cancel_run, is_terminal_workspace_status};
use crate::control::state::WorkingSet;
use ait_contracts::{
    ApiError, CommandResult, NativeApprovalAction, NativeApprovalView, NativePermissionProfile,
    ProtocolRequestId, RunView,
};
use ait_domain::{
    ApprovalGrantScope, DomainError, ErrorCode, NativeApprovalKind, NativeApprovalStatus,
    NativeApprovalTarget, RunPermissionProfile, SandboxAccess,
};
use ait_ports::{
    ControlStoreError, PendingEvent, WorkspaceApproval, WorkspaceApprovalDecision,
    WorkspaceApprovalRequest,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[async_trait::async_trait]
impl WorkspaceApproval for LocalControlService {
    async fn decide(
        &self,
        request: WorkspaceApprovalRequest,
    ) -> Result<WorkspaceApprovalDecision, DomainError> {
        let approval = native_approval_record(&request).map_err(api_domain_error)?;
        let approval_id = approval.id.clone();
        let cancellation = self
            .cancellations
            .lock()
            .ok()
            .and_then(|cancellations| cancellations.get(&request.run_id).cloned())
            .ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::RunNotResumable,
                    "native approval has no active Run supervisor",
                )
            })?;
        let (sender, mut receiver) = tokio::sync::watch::channel(None);
        {
            let mut waiters = self.approval_waiters.lock().map_err(|_| {
                DomainError::invariant(
                    ErrorCode::ToolApprovalRequired,
                    "native approval registry is unavailable",
                )
            })?;
            if waiters.contains_key(&approval_id) {
                return Err(DomainError::invariant(
                    ErrorCode::ToolCallDuplicate,
                    "duplicate native approval request",
                ));
            }
            waiters.insert(approval_id.clone(), sender);
        }
        let saved = match self.persist_native_approval(approval).await {
            Ok(saved) => saved,
            Err(failure) => {
                self.approval_waiters
                    .lock()
                    .ok()
                    .and_then(|mut waiters| waiters.remove(&approval_id));
                return Err(api_domain_error(failure));
            }
        };
        if saved.status != NativeApprovalStatus::Pending {
            self.approval_waiters
                .lock()
                .ok()
                .and_then(|mut waiters| waiters.remove(&approval_id));
            return approval_decision(&saved).ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::ToolApprovalRequired,
                    "native approval has no usable decision",
                )
            });
        }
        let decision = loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => break WorkspaceApprovalDecision::Cancelled,
                changed = receiver.changed() => {
                    if changed.is_err() {
                        break WorkspaceApprovalDecision::Cancelled;
                    }
                    if let Some(decision) = receiver.borrow_and_update().clone() {
                        break decision;
                    }
                }
            }
        };
        self.approval_waiters
            .lock()
            .ok()
            .and_then(|mut waiters| waiters.remove(&approval_id));
        Ok(decision)
    }

    async fn expire(&self, request: &WorkspaceApprovalRequest) -> Result<(), DomainError> {
        let approval_id = native_approval_id(&request.run_id, &request.protocol_request_id)
            .map_err(api_domain_error)?;
        for _ in 0..4 {
            let loaded = self
                .read_run_records(&request.run_id)
                .await
                .map_err(api_domain_error)?;
            let mut state = loaded.original.clone();
            let Some(run) = state.runs.iter_mut().find(|run| run.id == request.run_id) else {
                return Err(DomainError::invariant(
                    ErrorCode::InvalidRun,
                    "native approval Run not found",
                ));
            };
            let Some(approval) = run
                .native_approvals
                .iter_mut()
                .find(|approval| approval.id == approval_id)
            else {
                return Ok(());
            };
            if approval.status != NativeApprovalStatus::Pending {
                return Ok(());
            }
            approval.status = NativeApprovalStatus::Expired;
            approval.decided_at = Some(now());
            if !run
                .native_approvals
                .iter()
                .any(|candidate| candidate.status == NativeApprovalStatus::Pending)
                && !is_terminal_workspace_status(&run.status)
            {
                run.phase = Some("calling_agent".into());
            }
            let run = run.clone();
            let event = pending("run.approval_expired", Some(request.run_id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run);
                    return Ok(());
                }
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(api_domain_error(store_error(failure))),
            }
        }
        Err(DomainError::transient(
            ErrorCode::RunQueueConflict,
            "native approval expiry did not settle",
        ))
    }
}

fn native_approval_record(
    request: &WorkspaceApprovalRequest,
) -> Result<NativeApprovalView, ApiError> {
    if request.run_id.trim().is_empty()
        || request.thread_id.trim().is_empty()
        || request.turn_id.trim().is_empty()
        || request.item_id.trim().is_empty()
        || request.method.trim().is_empty()
        || request.run_id.len() > 512
        || request.thread_id.len() > 512
        || request.turn_id.len() > 512
        || request.item_id.len() > 512
        || request.method.len() > 128
        || request.run_id.contains('\0')
        || request.thread_id.contains('\0')
        || request.turn_id.contains('\0')
        || request.item_id.contains('\0')
        || request.method.contains('\0')
    {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "native approval correlation is incomplete",
            false,
        ));
    }
    let protocol_request_id = match &request.protocol_request_id {
        Value::String(value) if !value.is_empty() && value.len() <= 512 => {
            ProtocolRequestId::String(value.clone())
        }
        Value::Number(value) if value.is_i64() => {
            ProtocolRequestId::Integer(value.as_i64().expect("checked integer"))
        }
        _ => {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "native approval request id must be a bounded string or integer",
                false,
            ));
        }
    };
    validate_native_approval_target(request.kind, &request.target)?;
    let requested_permissions = if request.kind == NativeApprovalKind::Permissions {
        let value = request.requested_permissions.clone().ok_or_else(|| {
            error(
                ErrorCode::ToolApprovalRequired,
                "permission approval omitted its explicit permission profile",
                false,
            )
        })?;
        let profile: NativePermissionProfile = serde_json::from_value(value).map_err(|_| {
            error(
                ErrorCode::ToolApprovalRequired,
                "invalid Codex permission profile",
                false,
            )
        })?;
        validate_native_permission_profile(&profile)?;
        if serde_json::to_vec(&profile)
            .map_err(|_| {
                error(
                    ErrorCode::ToolApprovalRequired,
                    "Codex permission profile is not serializable",
                    false,
                )
            })?
            .len()
            > 128 * 1_024
        {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "Codex permission profile exceeds the persistence and UI limit",
                false,
            ));
        }
        Some(profile)
    } else {
        None
    };
    Ok(NativeApprovalView {
        id: native_approval_id(&request.run_id, &request.protocol_request_id)?,
        run_id: request.run_id.clone(),
        protocol_request_id,
        method: request.method.clone(),
        kind: request.kind,
        thread_id: request.thread_id.clone(),
        turn_id: request.turn_id.clone(),
        item_id: request.item_id.clone(),
        target: request.target.clone(),
        requested_permissions,
        status: NativeApprovalStatus::Pending,
        granted_scope: None,
        granted_permissions: None,
        created_at: now(),
        decided_at: None,
    })
}

fn native_approval_id(run_id: &str, request_id: &Value) -> Result<String, ApiError> {
    let request_id = serde_json::to_vec(request_id).map_err(|_| {
        error(
            ErrorCode::ToolApprovalRequired,
            "native approval request id is not serializable",
            false,
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(run_id.as_bytes());
    digest.update([0]);
    digest.update(request_id);
    Ok(format!("native-{:x}", digest.finalize()))
}

fn validate_native_approval_target(
    kind: NativeApprovalKind,
    target: &NativeApprovalTarget,
) -> Result<(), ApiError> {
    let valid = match (kind, target) {
        (
            NativeApprovalKind::CommandExecution | NativeApprovalKind::LegacyCommand,
            NativeApprovalTarget::Command { command, cwd },
        ) => is_bounded_display_value(command) && is_bounded_display_value(cwd),
        (NativeApprovalKind::CommandExecution, NativeApprovalTarget::Network { host, .. }) => {
            is_bounded_display_value(host)
        }
        (
            NativeApprovalKind::FileChange | NativeApprovalKind::LegacyPatch,
            NativeApprovalTarget::FileChange {
                grant_root,
                changes,
            },
        ) => {
            changes.len() <= 128
                && (!changes.is_empty() || grant_root.is_some())
                && grant_root.as_deref().is_none_or(is_bounded_display_value)
                && changes
                    .iter()
                    .all(|change| is_bounded_display_value(&change.path))
        }
        (NativeApprovalKind::Permissions, NativeApprovalTarget::Permissions { cwd }) => {
            is_bounded_display_value(cwd)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(error(
            ErrorCode::ToolApprovalRequired,
            "native approval has no bounded, reviewable authorization target",
            false,
        ))
    }
}

pub(in crate::control) fn is_bounded_display_value(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4_096 && !value.chars().any(char::is_control)
}

fn approval_decision(approval: &NativeApprovalView) -> Option<WorkspaceApprovalDecision> {
    match approval.status {
        NativeApprovalStatus::Approved => Some(WorkspaceApprovalDecision::Approved {
            scope: approval.granted_scope?,
            permissions: approval
                .granted_permissions
                .as_ref()
                .and_then(|profile| serde_json::to_value(profile).ok()),
        }),
        NativeApprovalStatus::Denied => Some(WorkspaceApprovalDecision::Denied),
        NativeApprovalStatus::Cancelled | NativeApprovalStatus::Expired => {
            Some(WorkspaceApprovalDecision::Cancelled)
        }
        NativeApprovalStatus::Pending => None,
    }
}

pub(in crate::control) fn resolve_native_approval(
    state: &mut WorkingSet,
    run_id: &str,
    approval_id: &str,
    action: NativeApprovalAction,
    scope: Option<ApprovalGrantScope>,
    limits: PermissionPolicyLimits,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let run_index = state
        .runs
        .iter()
        .position(|run| run.id == run_id)
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    let run = &state.runs[run_index];
    if is_terminal_workspace_status(&run.status) {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "native approval belongs to a terminal Run",
            false,
        ));
    }
    let approval_index = run
        .native_approvals
        .iter()
        .position(|approval| approval.id == approval_id)
        .ok_or_else(|| {
            error(
                ErrorCode::ToolApprovalRequired,
                "native approval request not found",
                false,
            )
        })?;
    if run.native_approvals[approval_index].status != NativeApprovalStatus::Pending {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "native approval request is no longer pending",
            false,
        ));
    }
    if action == NativeApprovalAction::Cancel {
        if scope.is_some() {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "cancellation cannot carry an authorization scope",
                false,
            ));
        }
        return cancel_run(state, run_id);
    }
    let project_root = state
        .projects
        .iter()
        .find(|project| project.id == run.project_id)
        .map(|project| PathBuf::from(&project.workdir))
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let run_profile = run.permission_profile;
    let approval = &mut state.runs[run_index].native_approvals[approval_index];
    match action {
        NativeApprovalAction::Approve => {
            let scope = scope.ok_or_else(|| {
                error(
                    ErrorCode::InvalidConfiguration,
                    "approval scope is required when approving",
                    false,
                )
            })?;
            validate_native_approval_grant(approval, scope, limits, run_profile, &project_root)?;
            approval.status = NativeApprovalStatus::Approved;
            approval.granted_scope = Some(scope);
            approval
                .granted_permissions
                .clone_from(&approval.requested_permissions);
        }
        NativeApprovalAction::Deny => {
            if scope.is_some() {
                return Err(error(
                    ErrorCode::InvalidConfiguration,
                    "denial cannot carry an authorization scope",
                    false,
                ));
            }
            approval.status = NativeApprovalStatus::Denied;
        }
        NativeApprovalAction::Cancel => unreachable!("handled as Run cancellation above"),
    }
    approval.decided_at = Some(now());
    let run = &mut state.runs[run_index];
    if !run
        .native_approvals
        .iter()
        .any(|candidate| candidate.status == NativeApprovalStatus::Pending)
    {
        run.phase = Some("calling_agent".into());
    }
    let run = run.clone();
    Ok((
        CommandResult::Run(run.clone()),
        vec![pending("run.approval_resolved", Some(run.id.clone()), &run)],
    ))
}

fn validate_native_approval_grant(
    approval: &NativeApprovalView,
    scope: ApprovalGrantScope,
    limits: PermissionPolicyLimits,
    run_profile: RunPermissionProfile,
    project_root: &Path,
) -> Result<(), ApiError> {
    if run_profile.sandbox > limits.max_sandbox {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "Run permission snapshot exceeds the administrator sandbox ceiling",
            false,
        ));
    }
    if scope == ApprovalGrantScope::Session && !limits.allow_session_approvals {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "session-scoped approvals are disabled by administrator policy",
            false,
        ));
    }
    validate_native_approval_target(approval.kind, &approval.target)?;
    match &approval.target {
        // The current command approval contract cannot prove that an accepted
        // shell command retains the filesystem sandbox. A cwd or redacted
        // command preview is not a capability boundary.
        NativeApprovalTarget::Command { .. }
            if run_profile.sandbox != SandboxAccess::FullAccess =>
        {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "command approval requires an explicitly selected full_access Run; sandbox confinement cannot be proven",
                false,
            ));
        }
        NativeApprovalTarget::FileChange {
            grant_root,
            changes,
        } => {
            if run_profile.sandbox == SandboxAccess::ReadOnly {
                return Err(error(
                    ErrorCode::InvalidConfiguration,
                    "file approval exceeds the read_only Run sandbox",
                    false,
                ));
            }
            if run_profile.sandbox == SandboxAccess::WorkspaceWrite {
                for path in grant_root
                    .iter()
                    .chain(changes.iter().map(|change| &change.path))
                {
                    ensure_permission_path_in_project(path, project_root)?;
                }
            }
        }
        _ => {}
    }
    if approval.kind == NativeApprovalKind::Permissions {
        let permissions = approval.requested_permissions.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "permission approval has no explicit permission profile",
                false,
            )
        })?;
        if scope == ApprovalGrantScope::OneShot {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex permission approvals support turn or session scope, not one-shot scope",
                false,
            ));
        }
        validate_permission_grant_ceiling(permissions, run_profile, limits, project_root)?;
    } else if scope == ApprovalGrantScope::Turn {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "Codex command and file approvals support one-shot or session scope, not turn scope",
            false,
        ));
    }
    Ok(())
}

fn validate_permission_grant_ceiling(
    permissions: &NativePermissionProfile,
    run_profile: RunPermissionProfile,
    limits: PermissionPolicyLimits,
    project_root: &Path,
) -> Result<(), ApiError> {
    let Some(file_system) = &permissions.file_system else {
        return Ok(());
    };
    let requests_write = !file_system.write.is_empty()
        || file_system
            .entries
            .iter()
            .any(|entry| entry.access == ait_contracts::NativeFileSystemAccess::Write);
    if requests_write
        && (run_profile.sandbox < SandboxAccess::WorkspaceWrite
            || limits.max_sandbox < SandboxAccess::WorkspaceWrite)
    {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "filesystem write grant exceeds the Run snapshot or administrator sandbox ceiling",
            false,
        ));
    }
    for path in file_system.read.iter().chain(&file_system.write) {
        ensure_permission_path_in_project(path, project_root)?;
    }
    for entry in &file_system.entries {
        match &entry.path {
            ait_contracts::NativeFileSystemPath::Path { path } => {
                ensure_permission_path_in_project(path, project_root)?;
            }
            ait_contracts::NativeFileSystemPath::Special {
                value: ait_contracts::NativeFileSystemSpecialPath::ProjectRoots { subpath },
            } => {
                if let Some(subpath) = subpath {
                    ensure_permission_path_in_project(subpath, project_root)?;
                }
            }
            ait_contracts::NativeFileSystemPath::GlobPattern { .. }
            | ait_contracts::NativeFileSystemPath::Special { .. } => {
                return Err(error(
                    ErrorCode::InvalidConfiguration,
                    "filesystem grant cannot be proven to stay inside the Project boundary",
                    false,
                ));
            }
        }
    }
    Ok(())
}

fn ensure_permission_path_in_project(path: &str, project_root: &Path) -> Result<(), ApiError> {
    use std::path::Component;

    let root = lexical_absolute_path(project_root)?;
    let candidate = Path::new(path);
    // Collapsing `symlink/..` lexically can hide an actual filesystem escape.
    if candidate
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return Err(project_boundary_error());
    }
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(project_boundary_error());
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    if !normalized.starts_with(&root) {
        return Err(project_boundary_error());
    }
    let canonical_root = std::fs::canonicalize(&root).map_err(|_| project_boundary_error())?;
    let mut existing = normalized.as_path();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(project_boundary_error)?;
            }
            Err(_) => return Err(project_boundary_error()),
        }
    }
    let canonical_existing =
        std::fs::canonicalize(existing).map_err(|_| project_boundary_error())?;
    if !canonical_existing.starts_with(canonical_root) {
        return Err(project_boundary_error());
    }
    Ok(())
}

fn lexical_absolute_path(path: &Path) -> Result<PathBuf, ApiError> {
    if !path.is_absolute() {
        return Err(project_boundary_error());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(project_boundary_error());
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn project_boundary_error() -> ApiError {
    error(
        ErrorCode::InvalidConfiguration,
        "filesystem grant escapes the Project boundary",
        false,
    )
}

pub(in crate::control) fn expire_pending_native_approvals(
    run: &mut RunView,
    status: NativeApprovalStatus,
) {
    let decided_at = now();
    for approval in &mut run.native_approvals {
        if approval.status == NativeApprovalStatus::Pending {
            approval.status = status;
            approval.decided_at = Some(decided_at);
        }
    }
}

impl LocalControlService {
    pub(in crate::control) fn notify_approval_waiters(&self, run: &RunView) {
        let Ok(mut waiters) = self.approval_waiters.lock() else {
            return;
        };
        for approval in &run.native_approvals {
            if approval.status == NativeApprovalStatus::Pending {
                continue;
            }
            let Some(sender) = waiters.remove(&approval.id) else {
                continue;
            };
            let decision =
                approval_decision(approval).unwrap_or(WorkspaceApprovalDecision::Cancelled);
            sender.send_replace(Some(decision));
        }
    }

    async fn persist_native_approval(
        &self,
        approval: NativeApprovalView,
    ) -> Result<NativeApprovalView, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(&approval.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == approval.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if is_terminal_workspace_status(&run.status) {
                return Err(error(
                    ErrorCode::RunAlreadyTerminal,
                    "native approval belongs to a terminal Run",
                    false,
                ));
            }
            if let Some(existing) = run
                .native_approvals
                .iter()
                .find(|existing| existing.id == approval.id)
            {
                if existing.protocol_request_id != approval.protocol_request_id
                    || existing.thread_id != approval.thread_id
                    || existing.turn_id != approval.turn_id
                    || existing.item_id != approval.item_id
                    || existing.target != approval.target
                    || existing.requested_permissions != approval.requested_permissions
                {
                    return Err(error(
                        ErrorCode::ToolCallDuplicate,
                        "native approval identity was reused with different correlation",
                        false,
                    ));
                }
                return Ok(existing.clone());
            }
            run.native_approvals.push(approval.clone());
            run.phase = Some("waiting_approval".into());
            let run = run.clone();
            let event = pending("run.approval_requested", Some(run.id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(approval),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "native approval request did not settle",
            true,
        ))
    }
}
