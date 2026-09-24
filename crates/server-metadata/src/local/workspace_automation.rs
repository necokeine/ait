//! Local `paseo.json` setup and script process runtime.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::ports::workspace_automation::{
    ScriptSnapshot, ScriptType, SetupCommandSnapshot, SetupLifecycle, SetupSnapshot,
    WorkspaceAutomationError, WorkspaceAutomationRuntime, WorkspacePlacement,
};

const CONFIG_BYTES: usize = 1024 * 1024;
const OUTPUT_BYTES: usize = 64 * 1024;
const CAPTURE_BYTES: u64 = 8 * 1024 * 1024;
const SETUP_TIMEOUT: Duration = Duration::from_mins(30);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const TRUNCATION_MARKER: &[u8] = b"\n... setup output truncated ...\n";

/// Process-backed workspace setup and script adapter.
#[derive(Debug, Clone, Default)]
pub struct LocalWorkspaceAutomation {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    state: Mutex<State>,
    sequence: AtomicU64,
}

#[derive(Debug, Default)]
struct State {
    scripts: HashMap<(String, String), ScriptProcess>,
    setups: HashMap<String, SetupSnapshot>,
    setup_running: HashSet<String>,
    workspace_ports: HashMap<String, u16>,
}

#[derive(Debug)]
struct ScriptProcess {
    kind: ScriptType,
    hostname: String,
    port: Option<u16>,
    terminal_id: String,
    child: Option<Child>,
    exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
struct ScriptConfig {
    command: String,
    kind: ScriptType,
    port: Option<u16>,
}

#[derive(Debug, Default)]
struct PaseoConfig {
    setup: Vec<String>,
    scripts: BTreeMap<String, ScriptConfig>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for process in state.scripts.values_mut() {
            if let Some(child) = process.child.as_mut() {
                let _ = terminate_child(child);
            }
        }
    }
}

impl WorkspaceAutomationRuntime for LocalWorkspaceAutomation {
    fn list_scripts(
        &self,
        workspace: &WorkspacePlacement,
    ) -> Result<Vec<ScriptSnapshot>, WorkspaceAutomationError> {
        let config = read_config(Path::new(&workspace.cwd))?;
        let mut state = lock(&self.inner.state);
        refresh_workspace_processes(&mut state, &workspace.workspace_id)?;
        let mut snapshots = Vec::with_capacity(config.scripts.len());
        for (name, configured) in &config.scripts {
            let key = (workspace.workspace_id.clone(), name.clone());
            snapshots.push(match state.scripts.get(&key) {
                Some(process) => process.snapshot(name),
                None => configured.snapshot(name),
            });
        }
        for ((workspace_id, name), process) in &state.scripts {
            if workspace_id == &workspace.workspace_id && !config.scripts.contains_key(name) {
                snapshots.push(process.snapshot(name));
            }
        }
        snapshots.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(snapshots)
    }

    fn start_script(
        &self,
        workspace: &WorkspacePlacement,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationError> {
        let config = read_config(Path::new(&workspace.cwd))?;
        let configured = config
            .scripts
            .get(script_name)
            .ok_or_else(|| WorkspaceAutomationError::UnknownScript(script_name.to_owned()))?;
        let key = (workspace.workspace_id.clone(), script_name.to_owned());
        let mut state = lock(&self.inner.state);
        if let Some(process) = state.scripts.get_mut(&key) {
            refresh_process(process)?;
            if process.child.is_some() {
                return Err(WorkspaceAutomationError::AlreadyRunning(
                    script_name.to_owned(),
                ));
            }
        }
        let workspace_port = workspace_port(&mut state, &workspace.workspace_id)?;
        let exposed_port = match configured.kind {
            ScriptType::Script => None,
            ScriptType::Service => Some(configured.port.unwrap_or(workspace_port)),
        };
        let terminal_id = format!(
            "terminal-{}-{}",
            std::process::id(),
            self.inner.sequence.fetch_add(1, Ordering::Relaxed)
        );
        let mut command = shell_command(&configured.command);
        configure_command(&mut command, workspace, workspace_port);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_process_group(&mut command);
        let child = command.spawn().map_err(|error| {
            WorkspaceAutomationError::Io(format!("Failed to start script '{script_name}': {error}"))
        })?;
        let process = ScriptProcess {
            kind: configured.kind,
            hostname: script_name.to_owned(),
            port: exposed_port,
            terminal_id,
            child: Some(child),
            exit_code: None,
        };
        let snapshot = process.snapshot(script_name);
        state.scripts.insert(key, process);
        Ok(snapshot)
    }

    fn stop_script(
        &self,
        workspace: &WorkspacePlacement,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationError> {
        let key = (workspace.workspace_id.clone(), script_name.to_owned());
        let mut state = lock(&self.inner.state);
        let process = state
            .scripts
            .get_mut(&key)
            .ok_or_else(|| WorkspaceAutomationError::NotRunning(script_name.to_owned()))?;
        refresh_process(process)?;
        let Some(mut child) = process.child.take() else {
            return Err(WorkspaceAutomationError::NotRunning(script_name.to_owned()));
        };
        let status = terminate_child(&mut child).map_err(|error| {
            WorkspaceAutomationError::Io(format!("Failed to reap script '{script_name}': {error}"))
        })?;
        process.exit_code = status.code();
        Ok(process.snapshot(script_name))
    }

    fn start_setup(
        &self,
        workspace: &WorkspacePlacement,
    ) -> Result<bool, WorkspaceAutomationError> {
        let config = match read_config(Path::new(&workspace.cwd)) {
            Ok(config) => config,
            Err(error) => {
                lock(&self.inner.state).setups.insert(
                    workspace.workspace_id.clone(),
                    failed_setup(workspace, error.to_string()),
                );
                return Err(error);
            }
        };
        let mut state = lock(&self.inner.state);
        if !state.setup_running.insert(workspace.workspace_id.clone()) {
            return Ok(false);
        }
        let port = match workspace_port(&mut state, &workspace.workspace_id) {
            Ok(port) => port,
            Err(error) => {
                state.setup_running.remove(&workspace.workspace_id);
                state.setups.insert(
                    workspace.workspace_id.clone(),
                    failed_setup(workspace, error.to_string()),
                );
                return Err(error);
            }
        };
        state.setups.insert(
            workspace.workspace_id.clone(),
            setup_snapshot(workspace, SetupLifecycle::Running),
        );
        drop(state);

        let inner = Arc::downgrade(&self.inner);
        let placement = workspace.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("workspace-setup-{}", workspace.workspace_id))
            .spawn(move || run_setup(&inner, &placement, config.setup, port))
        {
            let message = format!("Failed to start workspace setup: {error}");
            let mut state = lock(&self.inner.state);
            state.setup_running.remove(&workspace.workspace_id);
            state.setups.insert(
                workspace.workspace_id.clone(),
                failed_setup(workspace, message.clone()),
            );
            return Err(WorkspaceAutomationError::Io(message));
        }
        Ok(true)
    }

    fn setup_snapshot(&self, workspace_id: &str) -> Option<SetupSnapshot> {
        lock(&self.inner.state).setups.get(workspace_id).cloned()
    }
}

impl ScriptConfig {
    fn snapshot(&self, name: &str) -> ScriptSnapshot {
        ScriptSnapshot {
            name: name.to_owned(),
            kind: self.kind,
            hostname: name.to_owned(),
            port: self.port,
            running: false,
            exit_code: None,
            terminal_id: None,
        }
    }
}

impl ScriptProcess {
    fn snapshot(&self, name: &str) -> ScriptSnapshot {
        ScriptSnapshot {
            name: name.to_owned(),
            kind: self.kind,
            hostname: self.hostname.clone(),
            port: self.port,
            running: self.child.is_some(),
            exit_code: self.exit_code,
            terminal_id: Some(self.terminal_id.clone()),
        }
    }
}

fn read_config(root: &Path) -> Result<PaseoConfig, WorkspaceAutomationError> {
    let path = root.join("paseo.json");
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PaseoConfig::default());
        }
        Err(error) => return Err(config_error(&path, &error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(WorkspaceAutomationError::InvalidConfig(format!(
            "Failed to parse paseo.json at {}: expected a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|file| file.take((CONFIG_BYTES + 1) as u64).read_to_end(&mut bytes))
        .map_err(|error| config_error(&path, &error))?;
    if bytes.len() > CONFIG_BYTES {
        return Err(WorkspaceAutomationError::InvalidConfig(format!(
            "Failed to parse paseo.json at {}: file exceeds {CONFIG_BYTES} bytes",
            path.display()
        )));
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| config_error(&path, &error))?;
    let object = value.as_object().ok_or_else(|| {
        WorkspaceAutomationError::InvalidConfig(format!(
            "Failed to parse paseo.json at {}: expected an object",
            path.display()
        ))
    })?;
    Ok(PaseoConfig {
        setup: parse_setup(object.get("worktree")),
        scripts: parse_scripts(object.get("scripts")),
    })
}

fn parse_setup(worktree: Option<&Value>) -> Vec<String> {
    let Some(setup) = worktree
        .and_then(Value::as_object)
        .and_then(|worktree| worktree.get("setup"))
    else {
        return Vec::new();
    };
    match setup {
        Value::String(command) => nonempty(command).into_iter().collect(),
        Value::Array(commands) => commands
            .iter()
            .filter_map(Value::as_str)
            .filter_map(nonempty)
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_scripts(scripts: Option<&Value>) -> BTreeMap<String, ScriptConfig> {
    scripts
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            let object = value.as_object()?;
            let command = nonempty(object.get("command")?.as_str()?)?;
            let kind = if object.get("type").and_then(Value::as_str) == Some("service") {
                ScriptType::Service
            } else {
                ScriptType::Script
            };
            let port = (kind == ScriptType::Service)
                .then(|| object.get("port").and_then(Value::as_u64))
                .flatten()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0);
            Some((
                name.clone(),
                ScriptConfig {
                    command,
                    kind,
                    port,
                },
            ))
        })
        .collect()
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn config_error(path: &Path, error: &impl std::fmt::Display) -> WorkspaceAutomationError {
    WorkspaceAutomationError::InvalidConfig(format!(
        "Failed to parse paseo.json at {}: {error}",
        path.display()
    ))
}

fn refresh_workspace_processes(
    state: &mut State,
    workspace_id: &str,
) -> Result<(), WorkspaceAutomationError> {
    for ((entry_workspace_id, _), process) in &mut state.scripts {
        if entry_workspace_id == workspace_id {
            refresh_process(process)?;
        }
    }
    Ok(())
}

fn refresh_process(process: &mut ScriptProcess) -> Result<(), WorkspaceAutomationError> {
    let Some(child) = process.child.as_mut() else {
        return Ok(());
    };
    let status = child.try_wait().map_err(|error| {
        WorkspaceAutomationError::Io(format!("Failed to inspect script process: {error}"))
    })?;
    if let Some(status) = status {
        process.exit_code = status.code();
        process.child = None;
    }
    Ok(())
}

fn workspace_port(state: &mut State, workspace_id: &str) -> Result<u16, WorkspaceAutomationError> {
    if let Some(port) = state.workspace_ports.get(workspace_id) {
        return Ok(*port);
    }
    let port = TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map_err(|error| {
            WorkspaceAutomationError::Io(format!("Failed to allocate workspace port: {error}"))
        })?
        .port();
    state.workspace_ports.insert(workspace_id.to_owned(), port);
    Ok(port)
}

fn shell_command(script: &str) -> Command {
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command.args(["-lc", script]);
        command
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C", script]);
        command
    }
}

fn configure_command(command: &mut Command, workspace: &WorkspacePlacement, port: u16) {
    command
        .current_dir(&workspace.cwd)
        .env("PASEO_SOURCE_CHECKOUT_PATH", &workspace.repo_root)
        .env("PASEO_ROOT_PATH", &workspace.repo_root)
        .env("PASEO_WORKTREE_PATH", &workspace.worktree_path)
        .env("PASEO_BRANCH_NAME", &workspace.branch_name)
        .env("PASEO_WORKTREE_PORT", port.to_string());
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

fn setup_snapshot(workspace: &WorkspacePlacement, lifecycle: SetupLifecycle) -> SetupSnapshot {
    SetupSnapshot {
        lifecycle,
        worktree_path: workspace.worktree_path.clone(),
        branch_name: workspace.branch_name.clone(),
        log: String::new(),
        commands: Vec::new(),
        truncated: false,
        error: None,
    }
}

fn failed_setup(workspace: &WorkspacePlacement, error: String) -> SetupSnapshot {
    SetupSnapshot {
        lifecycle: SetupLifecycle::Failed,
        error: Some(error),
        ..setup_snapshot(workspace, SetupLifecycle::Failed)
    }
}

fn run_setup(
    inner: &Weak<Inner>,
    workspace: &WorkspacePlacement,
    commands: Vec<String>,
    port: u16,
) {
    let total = commands.len();
    let mut snapshots = Vec::with_capacity(total);
    let mut rendered = String::new();
    let mut truncated = false;
    let mut failure = None;
    for (offset, command) in commands.into_iter().enumerate() {
        let index = offset + 1;
        snapshots.push(SetupCommandSnapshot {
            index,
            command: command.clone(),
            cwd: workspace.cwd.clone(),
            log: String::new(),
            running: true,
            exit_code: None,
            duration_ms: None,
        });
        publish_setup(
            inner,
            workspace,
            SetupProgress {
                lifecycle: SetupLifecycle::Running,
                log: &rendered,
                commands: &snapshots,
                truncated,
                error: None,
            },
        );
        let started = Instant::now();
        match run_setup_command(inner, workspace, &command, port) {
            Ok((status, output, output_truncated)) => {
                let duration = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let snapshot = snapshots.last_mut().expect("setup command was appended");
                snapshot.running = false;
                snapshot.exit_code = status.code();
                snapshot.duration_ms = Some(duration);
                snapshot.log.clone_from(&output);
                truncated |= output_truncated;
                let _ = writeln!(rendered, "==> [{index}/{total}] {command}");
                rendered.push_str(&output);
                if !output.ends_with('\n') && !output.is_empty() {
                    rendered.push('\n');
                }
                if !status.success() {
                    failure = Some(format!(
                        "Setup command {index} failed{}",
                        status
                            .code()
                            .map_or_else(String::new, |code| format!(" with exit code {code}"))
                    ));
                    break;
                }
            }
            Err(error) => {
                let duration = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let snapshot = snapshots.last_mut().expect("setup command was appended");
                snapshot.running = false;
                snapshot.duration_ms = Some(duration);
                failure = Some(error.to_string());
                break;
            }
        }
    }
    let lifecycle = if failure.is_some() {
        SetupLifecycle::Failed
    } else {
        SetupLifecycle::Completed
    };
    publish_setup(
        inner,
        workspace,
        SetupProgress {
            lifecycle,
            log: &rendered,
            commands: &snapshots,
            truncated,
            error: failure,
        },
    );
    if let Some(inner) = inner.upgrade() {
        lock(&inner.state)
            .setup_running
            .remove(&workspace.workspace_id);
    }
}

#[derive(Debug)]
struct SetupProgress<'a> {
    lifecycle: SetupLifecycle,
    log: &'a str,
    commands: &'a [SetupCommandSnapshot],
    truncated: bool,
    error: Option<String>,
}

fn publish_setup(inner: &Weak<Inner>, workspace: &WorkspacePlacement, progress: SetupProgress<'_>) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    lock(&inner.state).setups.insert(
        workspace.workspace_id.clone(),
        SetupSnapshot {
            lifecycle: progress.lifecycle,
            worktree_path: workspace.worktree_path.clone(),
            branch_name: workspace.branch_name.clone(),
            log: progress.log.to_owned(),
            commands: progress.commands.to_vec(),
            truncated: progress.truncated,
            error: progress.error,
        },
    );
}

fn run_setup_command(
    inner: &Weak<Inner>,
    workspace: &WorkspacePlacement,
    script: &str,
    port: u16,
) -> Result<(ExitStatus, String, bool), WorkspaceAutomationError> {
    let mut capture = tempfile::tempfile().map_err(setup_io)?;
    let stdout = capture.try_clone().map_err(setup_io)?;
    let stderr = capture.try_clone().map_err(setup_io)?;
    let mut command = shell_command(script);
    configure_command(&mut command, workspace, port);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(setup_io)?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(setup_io)? {
            break status;
        }
        if inner.upgrade().is_none() {
            let _ = terminate_child(&mut child);
            return Err(WorkspaceAutomationError::Io(
                "Workspace setup was cancelled".to_owned(),
            ));
        }
        let capture_too_large = capture.metadata().map_err(setup_io)?.len() > CAPTURE_BYTES;
        if started.elapsed() >= SETUP_TIMEOUT || capture_too_large {
            let _ = terminate_child(&mut child);
            let reason = if capture_too_large {
                "Setup command output exceeded 8 MiB"
            } else {
                "Setup command exceeded 30 minutes"
            };
            return Err(WorkspaceAutomationError::Io(reason.to_owned()));
        }
        thread::sleep(POLL_INTERVAL);
    };
    capture.seek(SeekFrom::Start(0)).map_err(setup_io)?;
    let mut output = Vec::new();
    capture.read_to_end(&mut output).map_err(setup_io)?;
    let (output, truncated) = bound_output(&output);
    Ok((status, output, truncated))
}

fn bound_output(output: &[u8]) -> (String, bool) {
    if output.len() <= OUTPUT_BYTES {
        return (String::from_utf8_lossy(output).into_owned(), false);
    }
    let side = (OUTPUT_BYTES - TRUNCATION_MARKER.len()) / 2;
    let mut bounded = Vec::with_capacity(OUTPUT_BYTES);
    bounded.extend_from_slice(&output[..side]);
    bounded.extend_from_slice(TRUNCATION_MARKER);
    bounded.extend_from_slice(&output[output.len() - side..]);
    (String::from_utf8_lossy(&bounded).into_owned(), true)
}

#[allow(clippy::needless_pass_by_value)]
fn setup_io(error: std::io::Error) -> WorkspaceAutomationError {
    WorkspaceAutomationError::Io(format!("Workspace setup process failed: {error}"))
}

fn terminate_child(child: &mut Child) -> std::io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &process_group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", &process_group])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                return child.wait();
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
    #[cfg(not(unix))]
    {
        child.kill()?;
        child.wait()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
