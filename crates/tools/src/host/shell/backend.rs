//! Build and probe the same restricted command used for real tool execution.
#[cfg(not(target_os = "macos"))]
use crate::host::denied;
#[cfg(target_os = "linux")]
use crate::host::failed;
use ait_domain::{DomainError, SandboxAccess};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct ShellBackend {
    sandbox: Option<PathBuf>,
}

impl ShellBackend {
    pub(crate) fn detect(root: &Path, access: SandboxAccess) -> Option<Self> {
        if !cfg!(unix) || !Path::new("/bin/bash").is_file() {
            return None;
        }
        if access == SandboxAccess::FullAccess {
            return Some(Self { sandbox: None });
        }
        sandbox_candidates()
            .iter()
            .find_map(|path| Self::probe(root, access, Path::new(path), Duration::from_secs(3)))
    }

    fn probe(root: &Path, access: SandboxAccess, path: &Path, timeout: Duration) -> Option<Self> {
        let backend = Self {
            sandbox: Some(path.to_owned()),
        };
        let mut command = backend.command(root, access, root).ok()?;
        // No model input or host data is read by the probe. Exercise namespaces,
        // mounts, seccomp and the real shell, not just the binary's version flag.
        command
            .arg("exit 0")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().ok()?;
        let deadline = Instant::now() + timeout;
        let available = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break false,
            }
        };
        // A failed/hung probe is reaped before returning. Killing bubblewrap's
        // supervisor also tears down its private PID namespace.
        let _ = child.kill();
        let _ = child.wait();
        available.then_some(backend)
    }

    pub(crate) fn command(
        &self,
        root: &Path,
        access: SandboxAccess,
        cwd: &Path,
    ) -> Result<Command, DomainError> {
        let mut command = Command::new(self.sandbox.as_deref().unwrap_or(Path::new("/bin/bash")));
        if self.sandbox.is_some() {
            restricted(&mut command, root, access, cwd)?;
            command.arg("/bin/bash");
        }
        command
            .args(["--noprofile", "--norc", "-c"])
            .env_clear()
            .env(
                "PATH",
                if self.sandbox.is_some() {
                    "/usr/bin:/bin:/usr/sbin:/sbin".into()
                } else {
                    std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into())
                },
            )
            .env("HOME", root)
            .env("LANG", "C")
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if self.sandbox.is_none() {
            command.stdin(Stdio::null());
        }
        Ok(command)
    }
}

fn sandbox_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    return &["/usr/bin/sandbox-exec"];
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    return &["/usr/bin/bwrap", "/bin/bwrap"];
    #[cfg(not(any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )))]
    &[]
}

#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)] // Shares the fallible Linux builder's contract.
fn restricted(
    command: &mut Command,
    root: &Path,
    access: SandboxAccess,
    _cwd: &Path,
) -> Result<(), DomainError> {
    command.args(["-p", MACOS_POLICY]);
    // Paths are argv parameters, never interpolated into policy source.
    for (name, path) in [
        ("WORKSPACE", root.to_owned()),
        ("GIT_METADATA", root.join(".git")),
        ("AIT_METADATA", root.join(".ait")),
    ] {
        command.arg("-D").arg(format!("{name}={}", path.display()));
    }
    command.arg("-D").arg(format!(
        "WRITABLE={}",
        if access == SandboxAccess::WorkspaceWrite {
            "yes"
        } else {
            "no"
        }
    ));
    command.stdin(Stdio::null());
    Ok(())
}

#[cfg(target_os = "macos")]
const MACOS_POLICY: &str = r#"
(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
; Only the Session and OS-supplied executables/libraries are readable. In
; particular /Users, /Library, /private, /Volumes and /usr/local are not grants.
(allow file-read*
    (literal "/") ; dyld needs the root vnode; this does not grant descendants.
    (subpath (param "WORKSPACE"))
    (subpath "/bin") (subpath "/sbin")
    (subpath "/usr/bin") (subpath "/usr/sbin") (subpath "/usr/lib")
    (subpath "/System/Library")
    (literal "/dev/null") (literal "/dev/zero")
    (literal "/dev/random") (literal "/dev/urandom")
    (subpath "/dev/fd"))
(allow file-write-data (literal "/dev/null"))
(if (string=? (param "WRITABLE") "yes")
    (allow file-write* (subpath (param "WORKSPACE"))))
(deny file-write* (subpath (param "GIT_METADATA")) (subpath (param "AIT_METADATA")))
"#;

#[cfg(target_os = "linux")]
fn restricted(
    command: &mut Command,
    root: &Path,
    access: SandboxAccess,
    cwd: &Path,
) -> Result<(), DomainError> {
    // Start with bubblewrap's empty root, never a bind of the host root. Mount
    // only OS runtime directories, a fixed loader cache and this Session.
    // Home directories, other Projects, /etc secrets, /run and host /tmp stay absent.
    command.args(["--unshare-all", "--die-with-parent", "--new-session"]);
    for path in [
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/lib64",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc/ld.so.cache",
    ] {
        if Path::new(path).exists() {
            command.args(["--ro-bind", path, path]);
        }
    }
    command.args(["--proc", "/proc", "--dev", "/dev"]);
    command
        .arg(if access == SandboxAccess::WorkspaceWrite {
            "--bind"
        } else {
            "--ro-bind"
        })
        .arg(root)
        .arg(root);
    if access == SandboxAccess::WorkspaceWrite {
        for name in [".git", ".ait"] {
            let path = root.join(name);
            // Never import a symlink's external target as a metadata mount.
            // A symlink left in the Session still resolves only within the
            // isolated filesystem, where private host paths do not exist.
            if std::fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_symlink()) {
                command.arg("--ro-bind").arg(&path).arg(&path);
            }
        }
    }
    // Keep the synthetic root and ancestors read-only as well.
    command
        .args(["--remount-ro", "/", "--seccomp", "0", "--chdir"])
        .arg(cwd)
        .arg("--");
    command.stdin(Stdio::from(network_filter()?));
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn restricted(_: &mut Command, _: &Path, _: SandboxAccess, _: &Path) -> Result<(), DomainError> {
    Err(denied())
}

// Classic BPF, passed to bubblewrap via an anonymous file on stdin. A restricted
// child gets only stdin/stdout/stderr, never an inherited host socket or io_uring.
#[cfg(target_os = "linux")]
fn network_filter() -> Result<std::fs::File, DomainError> {
    use std::io::{Seek, Write};
    #[cfg(target_arch = "x86_64")]
    let architecture = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    let architecture = 0xc000_00b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let architecture = 0; // Such hosts never advertise restricted Bash.
    let mut instructions: Vec<(u16, u8, u8, u32)> = vec![
        (0x20, 0, 0, 4), // Load seccomp_data.arch.
        (0x15, 1, 0, architecture),
        (0x06, 0, 0, 0x8000_0000), // Kill unknown/compat ABIs.
        (0x20, 0, 0, 0),           // Load seccomp_data.nr.
        (0x54, 0, 0, 0xbfff_ffff), // Strip the x32 syscall bit before comparison.
    ];
    for number in [
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_io_uring_setup,
    ] {
        instructions.push((0x15, 0, 1, u32::try_from(number).map_err(|_| denied())?));
        instructions.push((0x06, 0, 0, 0x0005_0001)); // EPERM.
    }
    instructions.push((0x06, 0, 0, 0x7fff_0000)); // Allow other syscalls.
    let mut file = tempfile::tempfile().map_err(|_| failed())?;
    for (code, yes, no, value) in instructions {
        file.write_all(&code.to_ne_bytes()).map_err(|_| failed())?;
        file.write_all(&[yes, no]).map_err(|_| failed())?;
        file.write_all(&value.to_ne_bytes()).map_err(|_| failed())?;
    }
    file.rewind().map_err(|_| failed())?;
    Ok(file)
}

#[cfg(all(test, unix))]
mod tests;
