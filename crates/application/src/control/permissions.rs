//! Run permission snapshots and administrator-owned policy ceilings.
use crate::control::approvals::is_bounded_display_value;
use crate::control::errors::error;
use ait_contracts::{
    AgentMode, AgentProvider, ApiError, NativePermissionProfile, SettingsDocument,
};
use ait_domain::{ApprovalMode, DomainError, ErrorCode, RunPermissionProfile, SandboxAccess};
use serde_json::Value;

/// Administrator-owned ceiling applied before a Run or approval can have side effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionPolicyLimits {
    pub max_sandbox: SandboxAccess,
    pub allow_session_approvals: bool,
}

impl Default for PermissionPolicyLimits {
    fn default() -> Self {
        Self {
            max_sandbox: SandboxAccess::FullAccess,
            allow_session_approvals: true,
        }
    }
}

pub(in crate::control) fn effective_permission_profile(
    settings: &SettingsDocument,
    provider: &AgentProvider,
    limits: PermissionPolicyLimits,
) -> Result<RunPermissionProfile, ApiError> {
    let sandbox = match settings
        .0
        .get("permissions.sandbox")
        .and_then(Value::as_str)
    {
        Some("read_only" | "strict") => SandboxAccess::ReadOnly,
        Some("workspace_write") => SandboxAccess::WorkspaceWrite,
        Some("full_access") => SandboxAccess::FullAccess,
        Some(_) => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "unsupported Agent sandbox policy",
                false,
            ));
        }
        None => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Agent sandbox policy is missing",
                false,
            ));
        }
    };
    if sandbox > limits.max_sandbox {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            format!(
                "Agent sandbox policy {sandbox:?} exceeds the administrator limit {:?}",
                limits.max_sandbox
            ),
            false,
        ));
    }
    if provider.kind != AgentMode::Codex {
        return Ok(RunPermissionProfile {
            sandbox,
            approval: ApprovalMode::OnRequest,
        });
    }
    let approval = match settings
        .0
        .get("permissions.approval")
        .and_then(Value::as_str)
    {
        Some("on_request") => ApprovalMode::OnRequest,
        Some("untrusted_only") => ApprovalMode::UntrustedOnly,
        Some("always") => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex app-server cannot guarantee approval for every native operation; permissions.approval=always is unsupported",
                false,
            ));
        }
        Some(_) => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "unsupported Codex approval policy",
                false,
            ));
        }
        None => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex approval policy is missing",
                false,
            ));
        }
    };
    Ok(RunPermissionProfile { sandbox, approval })
}

pub(in crate::control) fn validate_run_permission_ceiling(
    profile: RunPermissionProfile,
    limits: PermissionPolicyLimits,
) -> Result<(), DomainError> {
    if profile.sandbox > limits.max_sandbox {
        return Err(DomainError::invariant(
            ErrorCode::InvalidConfiguration,
            "Run permission snapshot exceeds the administrator sandbox ceiling",
        ));
    }
    Ok(())
}

pub(in crate::control) fn validate_native_permission_profile(
    profile: &NativePermissionProfile,
) -> Result<(), ApiError> {
    let mut explicit = false;
    if let Some(network) = &profile.network {
        if network.enabled.is_none() {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "network permission must explicitly state enabled",
                false,
            ));
        }
        explicit = true;
    }
    if let Some(file_system) = &profile.file_system {
        if file_system.glob_scan_max_depth == Some(0) {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "filesystem glob scan depth must be positive",
                false,
            ));
        }
        if file_system.entries.len() > 128
            || file_system.read.len() > 128
            || file_system.write.len() > 128
        {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "filesystem permission profile is too large",
                false,
            ));
        }
        for value in file_system.read.iter().chain(&file_system.write) {
            validate_permission_path(value)?;
        }
        for entry in &file_system.entries {
            match &entry.path {
                ait_contracts::NativeFileSystemPath::Path { path } => {
                    validate_permission_path(path)?;
                }
                ait_contracts::NativeFileSystemPath::GlobPattern { pattern } => {
                    validate_permission_path(pattern)?;
                }
                ait_contracts::NativeFileSystemPath::Special { value } => match value {
                    ait_contracts::NativeFileSystemSpecialPath::ProjectRoots {
                        subpath: Some(subpath),
                    } => validate_permission_path(subpath)?,
                    ait_contracts::NativeFileSystemSpecialPath::Unknown { .. } => {
                        return Err(error(
                            ErrorCode::ToolApprovalRequired,
                            "unknown special filesystem permission path",
                            false,
                        ));
                    }
                    _ => {}
                },
            }
        }
        explicit |= !file_system.entries.is_empty()
            || !file_system.read.is_empty()
            || !file_system.write.is_empty();
    }
    if !explicit {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "permission profile does not contain an explicit capability",
            false,
        ));
    }
    Ok(())
}

fn validate_permission_path(value: &str) -> Result<(), ApiError> {
    if !is_bounded_display_value(value) {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "permission path is empty or outside size limits",
            false,
        ));
    }
    Ok(())
}
