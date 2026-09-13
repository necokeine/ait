//! Bounded PATH recovery for a macOS application launched outside a terminal.

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, User, getuid},
};
use tokio::{io::AsyncReadExt, process::Command};

const MAX_OUTPUT: usize = 64 * 1024;
const START: &[u8] = b"\0AIT_PATH\0";
const END: &[u8] = b"\0AIT_PATH_END\0";
// No user-controlled text is interpolated. printenv works for array-valued PATH
// in fish too, and the NUL markers separate PATH from startup/logout chatter.
const PROBE: &str =
    r"/usr/bin/printf '\0AIT_PATH\0'; /usr/bin/printenv PATH; /usr/bin/printf '\0AIT_PATH_END\0'";

pub(super) async fn recover() -> Option<OsString> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            User::from_uid(getuid())
                .ok()
                .flatten()
                .map(|user| user.shell)
        })
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    let mut command = shell_command(&shell);
    if let Some(home) = std::env::var_os("HOME") {
        command.current_dir(home);
    }
    let output = probe(command, Duration::from_secs(5)).await;
    let path = output.and_then(|output| parse_path(&output, std::env::var_os("PATH").as_deref()));
    if path.is_none() {
        // Shell output may contain secrets from startup scripts. Never log it.
        eprintln!("AIT: login-shell PATH unavailable; using inherited PATH");
    }
    path
}

fn shell_command(shell: &Path) -> Command {
    let mut command = Command::new(shell);
    // Interactive startup is required for nvm/fnm setups in .zshrc/.bashrc.
    command
        .args(["-ilc", PROBE])
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    command
}

struct ShellProcess {
    child: tokio::process::Child,
    group: Option<Pid>,
}

impl ShellProcess {
    fn terminate(&mut self) {
        if let Some(group) = self.group.take() {
            let _ = killpg(group, Signal::SIGKILL);
        }
        let _ = self.child.start_kill();
    }
}

impl Drop for ShellProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

async fn probe(mut command: Command, deadline: Duration) -> Option<Vec<u8>> {
    let child = command.spawn().ok()?;
    let group = Pid::from_raw(i32::try_from(child.id()?).ok()?);
    let mut process = ShellProcess {
        child,
        group: Some(group),
    };
    let stdout = process.child.stdout.take()?;
    let mut output = Vec::new();
    let result = tokio::time::timeout(deadline, async {
        stdout
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut output)
            .await?;
        if output.len() > MAX_OUTPUT {
            return Err(std::io::Error::other("shell output limit"));
        }
        process.child.wait().await
    })
    .await;
    // Also terminate descendants holding the pipe open, including on cancellation.
    process.terminate();
    let _ = process.child.wait().await;
    result.ok()?.ok()?.success().then_some(output)
}

fn parse_path(output: &[u8], inherited: Option<&OsStr>) -> Option<OsString> {
    let start = output
        .windows(START.len())
        .position(|window| window == START)?
        + START.len();
    let rest = &output[start..];
    let end = rest.windows(END.len()).position(|window| window == END)?;
    let path = OsString::from_vec(rest[..end].strip_suffix(b"\n")?.to_vec());
    let mut paths = Vec::new();
    for entry in std::env::split_paths(&path) {
        if entry.is_absolute() && !paths.contains(&entry) {
            paths.push(entry);
        }
    }
    if paths.is_empty() {
        return None;
    }
    // Keep shell ordering (e.g. the selected nvm version) and retain inherited
    // absolute locations. Relative/empty entries must not search a Run workdir.
    if let Some(inherited) = inherited {
        for entry in std::env::split_paths(inherited) {
            if entry.is_absolute() && !paths.contains(&entry) {
                paths.push(entry);
            }
        }
    }
    std::env::join_paths(paths).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_preserves_order_spaces_and_inherited_locations_without_startup_output() {
        let output = b"welcome\n\0AIT_PATH\0/opt/homebrew/bin:/Users/me/Node Versions/bin::.:relative:/usr/bin:/opt/homebrew/bin\n\0AIT_PATH_END\0goodbye";
        assert_eq!(
            parse_path(output, Some(OsStr::new("/bin:/usr/bin"))).unwrap(),
            "/opt/homebrew/bin:/Users/me/Node Versions/bin:/usr/bin:/bin"
        );
        for output in [
            b"no markers".as_slice(),
            b"\0AIT_PATH\0\n\0AIT_PATH_END\0",
            b"\0AIT_PATH\0.:relative\n\0AIT_PATH_END\0",
        ] {
            assert!(parse_path(output, None).is_none());
        }
    }

    #[tokio::test]
    async fn interactive_login_startup_recovers_only_path() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zprofile"), "export PATH=/usr/bin:/bin\n").unwrap();
        std::fs::write(home.path().join(".zshrc"), "echo banner\nexport PATH=\"$HOME/Node Versions/bin:$PATH\"\nexport AIT_PATH_TEST_SECRET=not-imported\n").unwrap();
        let mut command = shell_command(Path::new("/bin/zsh"));
        command
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin");
        let output = probe(command, Duration::from_secs(2)).await.unwrap();
        let path = parse_path(&output, None).unwrap();
        assert_eq!(
            std::env::split_paths(&path).next().unwrap(),
            home.path().join("Node Versions/bin")
        );
        assert!(
            !output
                .windows(b"not-imported".len())
                .any(|part| part == b"not-imported")
        );
    }

    #[tokio::test]
    async fn failed_and_oversized_shells_fall_back_without_returning_output() {
        for script in [
            "exit 1",
            "while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done",
        ] {
            // Use a controlled script, without invoking the user's real startup files.
            let mut fixture = Command::new("/bin/sh");
            fixture
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0)
                .kill_on_drop(true);
            assert!(probe(fixture, Duration::from_secs(2)).await.is_none());
        }
    }

    #[tokio::test]
    async fn timeout_reaps_shell_and_its_descendant() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        std::fs::write(
            directory.path().join(".zshrc"),
            "/bin/sleep 30 & echo $! > \"$HOME/pid\"; wait\n",
        )
        .unwrap();
        let mut command = shell_command(Path::new("/bin/zsh"));
        command
            .env_clear()
            .env("HOME", directory.path())
            .env("PATH", "/usr/bin:/bin");
        assert!(probe(command, Duration::from_millis(500)).await.is_none());
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // A killed descendant can briefly remain a zombie until launchd reaps it.
        let status = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .await
            .unwrap();
        let state = String::from_utf8(status.stdout).unwrap();
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "descendant still running: {state}"
        );
    }
}
