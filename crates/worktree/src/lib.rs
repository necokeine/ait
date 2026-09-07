//! Exact, shared Git worktree snapshots used by guarded Run recovery.

use std::{
    ffi::OsString,
    fs::{File, Metadata},
    io::Read as _,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

/// One exact Git worktree observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeSnapshot {
    /// HEAD observed immediately before status, absent for an unborn branch.
    pub head: Option<String>,
    /// Complete NUL-delimited porcelain response used by display projections.
    pub status: Vec<u8>,
    /// SHA-256 over HEAD, status, diffs, and typed untracked content.
    pub fingerprint: String,
}

/// Failure to obtain an unambiguous worktree snapshot.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// Git could not be started.
    #[error("cannot execute Git: {0}")]
    Spawn(#[source] std::io::Error),
    /// Git rejected an inspection command.
    #[error("Git worktree inspection failed: {0}")]
    Git(String),
    /// A changed path could not be read consistently.
    #[error("cannot inspect retained path {path}: {message}")]
    Path {
        /// Project-relative path reported by Git.
        path: PathBuf,
        /// Safe I/O failure text.
        message: String,
    },
    /// Git reported an untracked object that cannot be represented in a tree.
    #[error("retained path has unsupported file type: {0}")]
    UnsupportedType(PathBuf),
    /// A file changed while its bytes were being fingerprinted.
    #[error("retained path changed during inspection: {0}")]
    ConcurrentChange(PathBuf),
}

/// Reads HEAD, complete status, staged/unstaged diffs, and typed untracked data.
///
/// # Errors
///
/// Returns [`WorktreeError`] when Git or any reported path cannot be inspected
/// consistently.
pub fn inspect(root: &Path) -> Result<WorktreeSnapshot, WorktreeError> {
    let head = git_head(root)?;
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let fingerprint = fingerprint(root, head.as_deref(), &status)?;
    Ok(WorktreeSnapshot {
        head,
        status,
        fingerprint,
    })
}

fn fingerprint(root: &Path, head: Option<&str>, status: &[u8]) -> Result<String, WorktreeError> {
    let unstaged = git(root, &["diff", "--binary"])?;
    let staged = git(root, &["diff", "--cached", "--binary"])?;
    let mut digest = Sha256::new();
    update_part(&mut digest, b"head", head.unwrap_or_default().as_bytes());
    update_part(&mut digest, b"status", status);
    update_part(&mut digest, b"unstaged", &unstaged);
    update_part(&mut digest, b"staged", &staged);

    for record in status.split(|byte| *byte == 0) {
        if record.len() < 3 || &record[..2] != b"??" || record[2] != b' ' {
            continue;
        }
        let relative_bytes = &record[3..];
        let relative = relative_path(relative_bytes);
        let absolute = root.join(&relative);
        let before =
            std::fs::symlink_metadata(&absolute).map_err(|error| path_error(&relative, &error))?;
        update_part(&mut digest, b"untracked-path", relative_bytes);
        if before.file_type().is_symlink() {
            update_part(&mut digest, b"untracked-mode", b"120000");
            let target =
                std::fs::read_link(&absolute).map_err(|error| path_error(&relative, &error))?;
            update_part(&mut digest, b"untracked-content", &path_bytes(&target));
        } else if before.is_file() {
            let mode = if executable(&before) {
                b"100755"
            } else {
                b"100644"
            };
            update_part(&mut digest, b"untracked-mode", mode);
            update_file(&mut digest, &absolute, &relative, before.len())?;
        } else {
            return Err(WorktreeError::UnsupportedType(relative));
        }
        let after =
            std::fs::symlink_metadata(&absolute).map_err(|error| path_error(&relative, &error))?;
        if !same_observation(&before, &after) {
            return Err(WorktreeError::ConcurrentChange(relative));
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn update_file(
    digest: &mut Sha256,
    absolute: &Path,
    relative: &Path,
    expected_len: u64,
) -> Result<(), WorktreeError> {
    digest.update(
        u64::try_from(b"untracked-content".len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    digest.update(b"untracked-content");
    digest.update(expected_len.to_le_bytes());
    let mut file = File::open(absolute).map_err(|error| path_error(relative, &error))?;
    let mut observed = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| path_error(relative, &error))?;
        if read == 0 {
            break;
        }
        observed = observed.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        digest.update(&buffer[..read]);
    }
    if observed != expected_len {
        return Err(WorktreeError::ConcurrentChange(relative.to_path_buf()));
    }
    Ok(())
}

fn update_part(digest: &mut Sha256, tag: &[u8], value: &[u8]) {
    digest.update(u64::try_from(tag.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(tag);
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(value);
}

fn git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>, WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(WorktreeError::Spawn)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(WorktreeError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

fn git_head(root: &Path) -> Result<Option<String>, WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(WorktreeError::Spawn)?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

fn path_error(path: &Path, error: &std::io::Error) -> WorktreeError {
    WorktreeError::Path {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(unix)]
fn relative_path(value: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt as _;
    PathBuf::from(OsString::from_vec(value.to_vec()))
}

#[cfg(not(unix))]
fn relative_path(value: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(value).into_owned())
}

#[cfg(unix)]
fn path_bytes(value: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    value.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(value: &Path) -> Vec<u8> {
    value.as_os_str().to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &Metadata) -> bool {
    false
}

fn same_observation(before: &Metadata, after: &Metadata) -> bool {
    before.file_type() == after.file_type()
        && before.len() == after.len()
        && executable(before) == executable(after)
        && before.modified().ok() == after.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let project = tempfile::TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(project.path())
                .arg("init")
                .status()
                .unwrap()
                .success()
        );
        project
    }

    #[test]
    fn unchanged_snapshot_has_the_same_fingerprint() {
        let project = project();
        std::fs::write(project.path().join("new.txt"), "target").unwrap();
        assert_eq!(
            inspect(project.path()).unwrap(),
            inspect(project.path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn untracked_file_type_and_executable_mode_change_the_fingerprint() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let project = project();
        let path = project.path().join("new.txt");
        std::fs::write(&path, "target").unwrap();
        let regular = inspect(project.path()).unwrap().fingerprint;

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        let executable = inspect(project.path()).unwrap().fingerprint;
        assert_ne!(regular, executable);

        std::fs::remove_file(&path).unwrap();
        symlink("target", &path).unwrap();
        let symlinked = inspect(project.path()).unwrap().fingerprint;
        assert_ne!(regular, symlinked);
        assert_ne!(executable, symlinked);
    }
}
