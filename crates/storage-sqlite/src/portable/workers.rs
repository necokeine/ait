//! A spawned process group is journaled before its executable bootstrap is sent.
use super::{Connection, ControlStoreError, State, other, params, project, sql_error};
use ait_domain::ProjectOwner;

pub(super) fn register(
    state: &State,
    owner: &ProjectOwner,
    pid: u32,
) -> Result<(), ControlStoreError> {
    let connection = connection(state, owner)?;
    if state.draining.contains(&owner.project_id) {
        return Err(other("PROJECT_DRAINING: new workers are not admitted"));
    }
    assert_prior_quiescent(connection)?;
    if pid <= 1 {
        return Err(other("Invalid worker process group"));
    }
    connection
        .execute(
            "INSERT INTO worker_processes VALUES(?1,?2,?3,?4)",
            params![pid, owner.runtime_instance_id, owner.owner_epoch, boot_id()],
        )
        .map_err(sql_error)?;
    Ok(())
}

pub(super) fn release(
    state: &State,
    owner: &ProjectOwner,
    pid: u32,
) -> Result<(), ControlStoreError> {
    let connection = connection(state, owner)?;
    #[cfg(unix)]
    if group_exists(pid)? {
        return Err(other(
            "RECOVERY_BLOCKED: worker descendants have not exited",
        ));
    }
    connection.execute("DELETE FROM worker_processes WHERE pid=?1 AND runtime_instance_id=?2 AND owner_epoch=?3",params![pid,owner.runtime_instance_id,owner.owner_epoch]).map_err(sql_error)?;
    Ok(())
}

fn connection<'a>(
    state: &'a State,
    owner: &ProjectOwner,
) -> Result<&'a Connection, ControlStoreError> {
    let project = state
        .projects
        .get(&owner.project_id)
        .ok_or(ControlStoreError::Conflict)?;
    if project::version(&project.connection)?.owner(&owner.project_id) != *owner {
        return Err(ControlStoreError::Conflict);
    }
    Ok(&project.connection)
}

pub(super) fn assert_prior_quiescent(connection: &Connection) -> Result<(), ControlStoreError> {
    verify_groups(connection, false)
}

pub(super) fn assert_quiescent(connection: &Connection) -> Result<(), ControlStoreError> {
    verify_groups(connection, true)
}

fn verify_groups(connection: &Connection, closing: bool) -> Result<(), ControlStoreError> {
    let current = project::version(connection)?;
    let mut query = connection
        .prepare("SELECT pid,runtime_instance_id,owner_epoch,boot_id FROM worker_processes")
        .map_err(sql_error)?;
    let claims = query
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    for (pid, runtime, epoch, saved_boot) in claims {
        // A boot change proves that even descendants which created their own
        // process group have stopped. Absence of the worker PGID alone does not.
        if !saved_boot.is_empty() && !boot_id().is_empty() && saved_boot != boot_id() {
            connection
                .execute("DELETE FROM worker_processes WHERE pid=?1", [pid])
                .map_err(sql_error)?;
            continue;
        }
        let prior = runtime != current.runtime_instance_id || epoch != current.owner_epoch;
        if closing || prior || !group_exists(pid)? {
            return Err(other(
                "RECOVERY_BLOCKED: worker cleanup was not acknowledged; history is available, but execution remains blocked until a verified host restart. Do not delete lock files or worker receipts",
            ));
        }
    }
    Ok(())
}

fn boot_id() -> &'static str {
    static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BOOT.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.bootsessionuuid"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_owned())
                .unwrap_or_default()
        }
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .map(|value| value.trim().to_owned())
                .unwrap_or_default()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            String::new()
        }
    })
}

#[cfg(unix)]
fn group_exists(pid: u32) -> Result<bool, ControlStoreError> {
    let pid = i32::try_from(pid).map_err(|_| other("Invalid saved process group"))?;
    if pid <= 1 {
        return Err(other("Invalid saved process group"));
    }
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) | Err(nix::errno::Errno::EPERM) => Ok(true),
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Err(failure) => Err(other(format!(
            "Unable to verify prior worker process group: {failure}"
        ))),
    }
}

#[cfg(not(unix))]
fn group_exists(_pid: u32) -> Result<bool, ControlStoreError> {
    // Normal supervised exit clears the receipt after its Windows Job has drained.
    // Crash recovery needs a platform verifier, never a time-based guess.
    Ok(true)
}
