//! Bounded host tools. All filesystem handles descend from one Project capability.
use ait_domain::{DomainError, ErrorCode, RunPermissionProfile, SandboxAccess, ToolExecution};
use ait_ports::{RunTool, RunToolFactory, ToolInvocation, ToolOutcome, ToolRecovery};
use async_trait::async_trait;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    io::{Read, Write},
    path::{Component, Path},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

mod approval;
mod search;
mod shell;

/// Maximum input file, argument or output bytes accepted by host tools.
pub const MAX_BYTES: usize = 65_536;
/// Production factory using the immutable Run permission for supported tools.
#[derive(Default)]
pub struct HostToolFactory;
/// Observable filesystem boundaries; callbacks run on the I/O worker.
pub trait HostIoObserver: Send + Sync {
    /// Observe a boundary without changing tool arguments or filesystem authority.
    fn checkpoint(&self, execution_id: &str, point: HostIoCheckpoint);
}
/// Checkpoints used for host diagnostics and deterministic I/O fault injection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostIoCheckpoint {
    /// Before reading file contents.
    BeforeRead,
    /// Before publishing an atomic replacement.
    BeforePublish,
}
struct ObservedFactory(Arc<dyn HostIoObserver>);
impl HostToolFactory {
    /// Install an observer without replacing the production filesystem executor.
    pub fn with_observer(observer: Arc<dyn HostIoObserver>) -> impl RunToolFactory {
        ObservedFactory(observer)
    }
}
impl RunToolFactory for ObservedFactory {
    fn review(
        &self,
        root: &Path,
        profile: RunPermissionProfile,
        execution: &ToolExecution,
    ) -> Result<Option<ait_domain::ToolApprovalTarget>, DomainError> {
        approval::review(root, profile, execution)
    }
    fn create(
        &self,
        root: &Path,
        profile: RunPermissionProfile,
    ) -> Result<Arc<dyn RunTool>, DomainError> {
        Ok(Arc::new(HostTools {
            root: Arc::new(open_project_root(root)?),
            root_path: root.to_owned(),
            shell_backend: shell::ShellBackend::detect(root, profile.sandbox),
            profile,
            authority: None,
            observer: Some(self.0.clone()),
            workers: Arc::new(Workers::default()),
        }))
    }
}
impl RunToolFactory for HostToolFactory {
    fn review(
        &self,
        root: &Path,
        profile: RunPermissionProfile,
        execution: &ToolExecution,
    ) -> Result<Option<ait_domain::ToolApprovalTarget>, DomainError> {
        approval::review(root, profile, execution)
    }
    fn create(
        &self,
        root: &Path,
        profile: RunPermissionProfile,
    ) -> Result<Arc<dyn RunTool>, DomainError> {
        Ok(Arc::new(HostTools {
            root: Arc::new(open_project_root(root)?),
            root_path: root.to_owned(),
            shell_backend: shell::ShellBackend::detect(root, profile.sandbox),
            profile,
            authority: None,
            observer: None,
            workers: Arc::new(Workers::default()),
        }))
    }
}
fn open_project_root(root: &Path) -> Result<Dir, DomainError> {
    if !root.is_absolute() {
        return Err(denied());
    }
    let anchor = root.ancestors().last().ok_or_else(denied)?;
    let mut directory =
        Dir::open_ambient_dir(anchor, cap_std::ambient_authority()).map_err(|_| failed())?;
    for part in root.components() {
        match part {
            Component::Normal(name) => {
                directory = directory.open_dir_nofollow(name).map_err(|_| denied())?;
            }
            Component::RootDir | Component::Prefix(_) => {}
            _ => return Err(denied()),
        }
    }
    Ok(directory)
}

/// Narrows the bundled schema to the options implemented by this host slice.
#[must_use]
pub fn parameters(name: &str) -> Option<Value> {
    let catalog = crate::ToolSet::default();
    let mut schema = catalog.get(name)?.parameters.clone();
    let allowed: &[&str] = match name {
        "read" => &["file_path", "offset", "limit"],
        "write" => &[
            "file_path",
            "content",
            "sandbox_permissions",
            "justification",
        ],
        "edit" => &[
            "file_path",
            "old_string",
            "new_string",
            "replace_all",
            "sandbox_permissions",
            "justification",
        ],
        "grep" => &[
            "pattern",
            "path",
            "include",
            "output_mode",
            "offset",
            "limit",
        ],
        "glob" => &["pattern", "path", "offset", "limit"],
        "bash" => &[
            "command",
            "description",
            "timeoutMs",
            "workdir",
            "sandbox_permissions",
            "justification",
        ],
        _ => return None,
    };
    schema["properties"]
        .as_object_mut()?
        .retain(|key, _| allowed.contains(&key.as_str()));
    if matches!(name, "grep" | "glob") {
        schema["properties"]["offset"] =
            json!({"type":"integer","minimum":0,"description":"Zero-based result offset."});
        schema["properties"]["limit"] = json!({"type":"integer","minimum":1,"maximum":1000,"description":"Maximum returned results, default 200."});
    }
    if name == "grep" {
        schema["properties"]["output_mode"] = json!({"type":"string","enum":["content","count","files_with_matches"],"description":"content (default), per-file matching line counts, or matching paths. count avoids returning file contents."});
    }
    if let Some(permission) = schema["properties"].get_mut("sandbox_permissions") {
        permission["description"] = json!(
            "Optional requested access. Higher access requires interactive approval for this operation only, subject to administrator and tool limits. Provide justification. Approval never changes the Run baseline."
        );
    }
    schema["additionalProperties"] = Value::Bool(false);
    Some(schema)
}

#[derive(Clone)]
struct HostTools {
    root: Arc<Dir>,
    root_path: std::path::PathBuf,
    profile: RunPermissionProfile,
    authority: Option<ait_domain::ToolGrant>,
    shell_backend: Option<shell::ShellBackend>,
    observer: Option<Arc<dyn HostIoObserver>>,
    workers: Arc<Workers>,
}

#[derive(Default)]
struct Workers {
    grants: std::sync::Mutex<std::collections::HashSet<String>>,
    stopping: CancellationToken,
    active: AtomicUsize,
    done: tokio::sync::Notify,
}
struct WorkerGuard(Arc<Workers>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.done.notify_waiters();
    }
}
struct TemporaryFile<'a>(&'a Dir, &'a str);
impl Drop for TemporaryFile<'_> {
    fn drop(&mut self) {
        let _ = self.0.remove_file(self.1);
    }
}
fn failed() -> DomainError {
    DomainError::invariant(
        ErrorCode::ToolExecutionFailed,
        "tool failed or exceeded its bounded input/output contract",
    )
}
fn denied() -> DomainError {
    DomainError::invariant(
        ErrorCode::ToolApprovalRequired,
        "tool request exceeds the fixed sandbox or supported capability",
    )
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str, DomainError> {
    v.get(key).and_then(Value::as_str).ok_or_else(failed)
}
fn safe_component(value: &str) -> bool {
    !value.starts_with('.') && !value.eq_ignore_ascii_case("node_modules")
}
impl HostTools {
    fn check(&self, request: &ToolInvocation) -> Result<(), DomainError> {
        if request.cancellation.is_cancelled() || self.workers.stopping.is_cancelled() {
            return Err(DomainError::invariant(
                ErrorCode::RunCancelled,
                "host I/O cancelled before the next effect boundary",
            ));
        }
        if let Some(grant) = &self.authority {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| denied())?
                .as_millis();
            if i64::try_from(now).map_err(|_| denied())? >= grant.expires_at {
                return Err(denied());
            }
            approval::recheck_paths(&self.root_path, &grant.target)?;
        }
        Ok(())
    }
    fn checkpoint(
        &self,
        request: &ToolInvocation,
        point: HostIoCheckpoint,
    ) -> Result<(), DomainError> {
        self.check(request)?;
        if let Some(observer) = &self.observer {
            observer.checkpoint(request.execution_id.as_str(), point);
        }
        self.check(request)
    }
    fn parent(&self, path: &str, request: &ToolInvocation) -> Result<(Dir, String), DomainError> {
        self.check(request)?;
        let mut parts = Vec::new();
        for part in Path::new(path).components() {
            match part {
                Component::Normal(name) => {
                    let name = name
                        .to_str()
                        .filter(|s| safe_component(s))
                        .ok_or_else(denied)?;
                    parts.push(name.to_owned());
                }
                Component::CurDir => {}
                _ => return Err(denied()),
            }
        }
        let name = parts.pop().ok_or_else(denied)?;
        let mut dir = self.root.try_clone().map_err(|_| failed())?;
        for part in parts {
            self.check(request)?;
            dir = dir.open_dir_nofollow(part).map_err(|_| denied())?;
        }
        Ok((dir, name))
    }
    fn read(&self, path: &str, request: &ToolInvocation) -> Result<String, DomainError> {
        self.checkpoint(request, HostIoCheckpoint::BeforeRead)?;
        let (dir, name) = self.parent(path, request)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let mut file = dir.open_with(name, &options).map_err(|_| failed())?;
        if !file.metadata().map_err(|_| failed())?.is_file() {
            return Err(denied());
        }
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            self.check(request)?;
            let count = file.read(&mut buffer).map_err(|_| failed())?;
            if count == 0 {
                break;
            }
            if bytes.len() + count > MAX_BYTES {
                return Err(failed());
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        self.check(request)?;
        String::from_utf8(bytes).map_err(|_| failed())
    }
    fn write(
        &self,
        path: &str,
        content: &str,
        request: &ToolInvocation,
    ) -> Result<(), DomainError> {
        if self.profile.sandbox == SandboxAccess::ReadOnly {
            return Err(denied());
        }
        if content.len() > MAX_BYTES {
            return Err(failed());
        }
        let (dir, name) = self.parent(path, request)?;
        if dir
            .symlink_metadata(&name)
            .is_ok_and(|m| !m.is_file() || m.is_symlink())
        {
            return Err(denied());
        }
        // Replace atomically; never truncate an existing hard link or follow a symlink.
        let temporary = format!(".ait-tool-{}", request.execution_id.as_str());
        self.check(request)?;
        let mut file = dir
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
            .map_err(|_| failed())?;
        let _cleanup = TemporaryFile(&dir, &temporary);
        for chunk in content.as_bytes().chunks(8192) {
            self.check(request)?;
            file.write_all(chunk).map_err(|_| failed())?;
        }
        self.check(request)?;
        file.sync_all().map_err(|_| failed())?;
        self.checkpoint(request, HostIoCheckpoint::BeforePublish)?;
        dir.rename(&temporary, &dir, name).map_err(|_| failed())
    }
    fn filesystem(&self, request: &ToolInvocation) -> Result<Value, DomainError> {
        self.check(request)?;
        let args = &request.arguments;
        match request.tool_name.as_str() {
            "read" => self.read_window(request),
            "grep" | "glob" => self.search(request),
            "write" => {
                self.write(
                    string(args, "file_path")?,
                    string(args, "content")?,
                    request,
                )?;
                Ok(json!({"written":true}))
            }
            "edit" => {
                let path = string(args, "file_path")?;
                let text = self.read(path, request)?;
                let old = string(args, "old_string")?;
                let new = string(args, "new_string")?;
                let count = text.matches(old).count();
                if old.is_empty()
                    || count == 0
                    || (count != 1 && args.get("replace_all") != Some(&Value::Bool(true)))
                {
                    return Err(failed());
                }
                let expanded = text
                    .len()
                    .checked_sub(count.checked_mul(old.len()).ok_or_else(failed)?)
                    .and_then(|remaining| remaining.checked_add(count.checked_mul(new.len())?))
                    .ok_or_else(failed)?;
                if expanded > MAX_BYTES {
                    return Err(failed());
                }
                self.write(path, &text.replace(old, new), request)?;
                Ok(json!({"replaced":count}))
            }
            _ => Err(denied()),
        }
    }
}
#[async_trait]
impl RunTool for HostTools {
    fn executable_tools(&self) -> Vec<String> {
        let mut names = vec!["read", "grep", "glob"];
        if self.shell_available() {
            names.push("bash");
        }
        names.extend(["write", "edit"]);
        names.into_iter().map(str::to_owned).collect()
    }
    fn parallel_safe(&self, name: &str, args: &Value) -> bool {
        matches!(name, "read" | "grep" | "glob") && !self.requires_approval(name, args)
    }
    fn requires_approval(&self, name: &str, args: &Value) -> bool {
        approval::requested(name, args, self.profile.sandbox)
            .is_none_or(|requested| requested > self.profile.sandbox)
    }
    async fn execute_granted(
        &self,
        request: ToolInvocation,
        grant: ait_domain::ToolGrant,
    ) -> Result<ToolOutcome, DomainError> {
        self.execute_with_grant(request, grant).await
    }
    async fn execute(&self, mut request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        request.cancellation = request.cancellation.child_token();
        let _cancel_on_drop = request.cancellation.clone().drop_guard();
        if request.cancellation.is_cancelled() {
            return Err(DomainError::invariant(
                ErrorCode::RunCancelled,
                "tool cancelled",
            ));
        }
        if !self.executable_tools().contains(&request.tool_name)
            || self.requires_approval(&request.tool_name, &request.arguments)
            || request
                .arguments
                .get("run_in_background")
                .is_some_and(|v| v != &Value::Bool(false))
        {
            return Err(denied());
        }
        let schema = parameters(&request.tool_name).ok_or_else(failed)?;
        if !jsonschema::validator_for(&schema)
            .map_err(|_| failed())?
            .is_valid(&request.arguments)
        {
            return Err(failed());
        }
        if request.arguments.to_string().len() > MAX_BYTES {
            return Err(failed());
        }
        self.workers.active.fetch_add(1, Ordering::SeqCst);
        let guard = WorkerGuard(self.workers.clone());
        let host = self.clone();
        let output = if request.tool_name == "bash" {
            tokio::spawn(async move {
                let _guard = guard;
                host.shell(&request).await
            })
            .await
            .map_err(|_| failed())??
        } else {
            tokio::task::spawn_blocking(move || {
                let _guard = guard;
                host.filesystem(&request)
            })
            .await
            .map_err(|_| failed())??
        };
        if output.to_string().len() > MAX_BYTES {
            return Err(failed());
        }
        Ok(ToolOutcome { output })
    }
    async fn cancel_and_drain(&self) {
        self.workers.stopping.cancel();
        loop {
            let notified = self.workers.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.workers.active.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
    }
    async fn reconcile(&self, _: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }
}
