use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use server_ports::ProjectError;

// Git commands here are read-only, fixed arguments, with bounded output and lifetime.
pub(super) fn run(root: &Path, arguments: &[&str]) -> Result<String, ProjectError> {
    let mut output = tempfile::tempfile().map_err(|_| ProjectError::Io)?;
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false"])
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(output.try_clone().map_err(|_| ProjectError::Io)?);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    let mut child = command.spawn().map_err(|_| ProjectError::Io)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if Instant::now() < deadline
                    && output
                        .metadata()
                        .is_ok_and(|metadata| metadata.len() <= 16 * 1024) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProjectError::Io);
            }
        }
    };
    if !status.success() {
        return Err(ProjectError::UnsupportedWorkspace);
    }
    output
        .seek(SeekFrom::Start(0))
        .map_err(|_| ProjectError::Io)?;
    let mut text = String::new();
    output
        .take(16 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| ProjectError::Io)?;
    if text.len() > 16 * 1024 {
        return Err(ProjectError::UnsupportedWorkspace);
    }
    Ok(text.strip_suffix('\n').unwrap_or(&text).to_owned())
}
