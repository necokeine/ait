//! Review and apply explicit, invocation-bound grants using the real host backend.
use super::{
    HostTools, denied, failed, open_project_root, parameters, safe_component, shell, string,
};
use ait_domain::{
    DomainError, RunPermissionProfile, SandboxAccess, ToolApprovalTarget, ToolExecution, ToolGrant,
};
use ait_ports::{RunTool, ToolInvocation, ToolOutcome};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path},
    sync::Arc,
};

pub(super) fn requested(name: &str, args: &Value, current: SandboxAccess) -> Option<SandboxAccess> {
    let minimum = if matches!(name, "write" | "edit") {
        SandboxAccess::WorkspaceWrite
    } else {
        current
    };
    let explicit = match args.get("sandbox_permissions") {
        None => current,
        Some(Value::String(value)) => match value.as_str() {
            "workspace-write" => SandboxAccess::WorkspaceWrite,
            "danger-full-access" => SandboxAccess::FullAccess,
            _ => return None,
        },
        _ => return None,
    };
    Some(minimum.max(explicit))
}

fn bounded(text: &str) -> Result<String, DomainError> {
    if text.trim().is_empty()
        || text.len() > 4096
        || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(denied());
    }
    Ok(text.into())
}

// No symlinks, including ancestors. Record device/inode identities so replacing
// a directory or reviewed file while the user is deciding invalidates authority.
fn identity(path: &Path, allow_missing_leaf: bool) -> Result<String, DomainError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut digest = Sha256::new();
        let mut cursor = std::path::PathBuf::new();
        for component in path.components() {
            match component {
                Component::RootDir | Component::Normal(_) => cursor.push(component),
                _ => return Err(denied()),
            }
            match std::fs::symlink_metadata(&cursor) {
                Ok(meta) if !meta.is_symlink() && (meta.is_file() || meta.is_dir()) => {
                    digest.update(meta.dev().to_le_bytes());
                    digest.update(meta.ino().to_le_bytes());
                    if meta.is_file() {
                        digest.update(meta.len().to_le_bytes());
                        digest.update(meta.mtime().to_le_bytes());
                        digest.update(meta.mtime_nsec().to_le_bytes());
                        digest.update(meta.ctime().to_le_bytes());
                        digest.update(meta.ctime_nsec().to_le_bytes());
                    }
                }
                Err(e)
                    if allow_missing_leaf
                        && cursor == path
                        && e.kind() == std::io::ErrorKind::NotFound =>
                {
                    digest.update(b"absent");
                }
                _ => return Err(denied()),
            }
        }
        Ok(format!("{:x}", digest.finalize()))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, allow_missing_leaf);
        Err(denied())
    }
}

pub(super) fn review(
    root: &Path,
    profile: RunPermissionProfile,
    execution: &ToolExecution,
) -> Result<Option<ToolApprovalTarget>, DomainError> {
    review_arguments(root, profile, &execution.tool_name, &execution.arguments)
}

fn review_arguments(
    root: &Path,
    profile: RunPermissionProfile,
    name: &str,
    args: &Value,
) -> Result<Option<ToolApprovalTarget>, DomainError> {
    let requested = requested(name, args, profile.sandbox).ok_or_else(denied)?;
    if requested <= profile.sandbox {
        return Ok(None);
    }
    let schema = parameters(name).ok_or_else(denied)?;
    if !matches!(name, "write" | "edit" | "bash")
        || !jsonschema::validator_for(&schema)
            .map_err(|_| failed())?
            .is_valid(args)
        || args.to_string().len() > super::MAX_BYTES
    {
        return Err(denied());
    }
    let root_identity = identity(root, false)?;
    let _root = open_project_root(root)?;
    let (cwd, operation, target_identity) = if name == "bash" {
        let path = args.get("workdir").and_then(Value::as_str).unwrap_or(".");
        // Resolve lexical '.' only; reject parent components and symlinks before canonicalization.
        let joined = root.join(path);
        let clean: std::path::PathBuf = joined
            .components()
            .filter(|p| *p != Component::CurDir)
            .collect();
        let target_identity = identity(&clean, false)?;
        let cwd = clean.canonicalize().map_err(|_| denied())?;
        if !cwd.is_dir()
            || (requested != SandboxAccess::FullAccess && !cwd.starts_with(root))
            || shell::ShellBackend::detect(root, requested).is_none()
        {
            return Err(denied());
        }
        (cwd, bounded(string(args, "command")?)?, target_identity)
    } else {
        let path = Path::new(string(args, "file_path")?);
        if path.components().next().is_none()
            || path.components().any(
                |p| !matches!(p, Component::Normal(n) if n.to_str().is_some_and(safe_component)),
            )
        {
            return Err(denied());
        }
        let target = root.join(path);
        if std::fs::symlink_metadata(&target).is_ok_and(|meta| !meta.is_file() || meta.is_symlink())
        {
            return Err(denied());
        }
        let target_identity = identity(&target, name == "write")?;
        (
            root.to_owned(),
            bounded(target.to_str().ok_or_else(denied)?)?,
            target_identity,
        )
    };
    let reason = match args.get("justification") {
        Some(Value::String(reason)) => bounded(reason)?,
        None => "This operation requires more access than the Run baseline.".into(),
        _ => return Err(denied()),
    };
    Ok(Some(ToolApprovalTarget {
        tool_name: name.into(),
        cwd: bounded(cwd.to_str().ok_or_else(denied)?)?,
        operation,
        reason,
        current: profile.sandbox,
        requested,
        path_fingerprint: format!("{root_identity}:{target_identity}"),
    }))
}

pub(super) fn recheck_paths(root: &Path, target: &ToolApprovalTarget) -> Result<(), DomainError> {
    let path = if target.tool_name == "bash" {
        &target.cwd
    } else {
        &target.operation
    };
    let fingerprint = format!(
        "{}:{}",
        identity(root, false)?,
        identity(Path::new(path), target.tool_name == "write")?
    );
    if fingerprint != target.path_fingerprint {
        return Err(denied());
    }
    Ok(())
}

impl HostTools {
    pub(super) async fn execute_with_grant(
        &self,
        request: ToolInvocation,
        grant: ToolGrant,
    ) -> Result<ToolOutcome, DomainError> {
        let host = self.clone();
        let args = request.arguments.clone();
        let name = request.tool_name.clone();
        let target = tokio::task::spawn_blocking(move || {
            review_arguments(&host.root_path, host.profile, &name, &args)
        })
        .await
        .map_err(|_| failed())??
        .ok_or_else(denied)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| denied())?
            .as_millis();
        if grant.run_id != request.run_id.as_str()
            || grant.call_id != request.call_id
            || grant.execution_id != request.execution_id.as_str()
            || grant.target != target
            || grant.arguments_digest
                != format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&request.arguments).map_err(|_| failed())?)
                )
            || i64::try_from(now).map_err(|_| denied())? >= grant.expires_at
        {
            return Err(denied());
        }
        // Local clone only. All later invocations still use the original Run profile.
        if !self
            .workers
            .grants
            .lock()
            .map_err(|_| denied())?
            .insert(grant.request_id.clone())
        {
            return Err(denied());
        }
        let mut effective = self.clone();
        effective.profile.sandbox = target.requested;
        effective.authority = Some(grant);
        effective.root = Arc::new(open_project_root(&self.root_path)?);
        let root = self.root_path.clone();
        effective.shell_backend = if request.tool_name == "bash" {
            tokio::task::spawn_blocking(move || {
                shell::ShellBackend::detect(&root, target.requested)
            })
            .await
            .map_err(|_| failed())?
        } else {
            effective.shell_backend
        };
        effective.execute(request).await
    }
}
