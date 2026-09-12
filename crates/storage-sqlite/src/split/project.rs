use std::{fs, io::Write, path::Path, process::Command};

use super::{
    Connection, ControlStoreError, ProjectBatch, ProjectTarget, Transaction, apply_change,
    check_foreign_keys, io_error, json_error, other, params, pragma_number, sql_error, table_count,
};

pub(super) const PROJECT_APPLICATION_ID: u32 = 0x4149_5031; // AIP1

pub(super) fn open_project(
    target: &ProjectTarget,
    owner: &str,
    create: bool,
) -> Result<Connection, ControlStoreError> {
    let root = Path::new(&target.workdir);
    if !root.is_absolute() || !root.is_dir() {
        return Err(other(format!(
            "Project {} is unavailable: {}",
            target.id,
            root.display()
        )));
    }
    let directory = root.join(".ait");
    reject_symlink(&directory)?;
    let path = directory.join("project.sqlite3");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        reject_symlink(&directory.join(format!("project.sqlite3{suffix}")))?;
    }
    if create {
        exclude_history(root)?;
        if !directory.exists() {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory).map_err(io_error)?;
        }
    }
    let flags = if create {
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
    } else {
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let mut connection = Connection::open_with_flags(&path, flags).map_err(|error| {
        other(format!(
            "Project {} database is unavailable: {error}",
            target.id
        ))
    })?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(sql_error)?;
    let version = pragma_number(&connection, "user_version")?;
    let application = pragma_number(&connection, "application_id")?;
    if create && version == 0 && application == 0 && table_count(&connection)? == 0 {
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
            )
            .map_err(sql_error)?;
        let transaction = connection.transaction().map_err(sql_error)?;
        // Journal configuration must precede the schema transaction.
        let schema = include_str!("project.sql")
            .splitn(4, ';')
            .nth(3)
            .expect("project schema");
        transaction.execute_batch(schema).map_err(sql_error)?;
        transaction.execute(
            "INSERT INTO project_identity(singleton, project_id, coordinator_id) VALUES(1, ?1, ?2)",
            params![target.id, owner],
        ).map_err(sql_error)?;
        transaction
            .pragma_update(None, "application_id", PROJECT_APPLICATION_ID)
            .map_err(sql_error)?;
        transaction
            .pragma_update(None, "user_version", 1)
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        #[cfg(unix)]
        {
            fs::File::open(&directory)
                .map_err(io_error)?
                .sync_all()
                .map_err(io_error)?;
            fs::File::open(root)
                .map_err(io_error)?
                .sync_all()
                .map_err(io_error)?;
        }
    } else if version != 1 || application != PROJECT_APPLICATION_ID {
        return Err(other(format!(
            "unsupported Project database format for {} (application={application}, version={version})",
            target.id
        )));
    }
    let (identity, coordinator): (String, String) = connection
        .query_row(
            "SELECT project_id, coordinator_id FROM project_identity WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    if identity != target.id || coordinator != owner {
        return Err(other(format!(
            "Project database identity/coordinator mismatch for {}; use an explicit import into a new directory",
            target.id
        )));
    }
    connection
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(sql_error)?;
    Ok(connection)
}

fn exclude_history(root: &Path) -> Result<(), ControlStoreError> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    if fs::canonicalize(top.trim()).map_err(io_error)?
        != fs::canonicalize(root).map_err(io_error)?
    {
        return Err(other("Project workdir must be a Git root"));
    }
    if !git(root, &["ls-files", "--", ".ait"])?.is_empty() {
        return Err(other(
            "Project .ait contains tracked files; refusing to store history in Git",
        ));
    }
    let exclude = git(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    )?;
    let exclude = Path::new(exclude.trim());
    let existing = match fs::read_to_string(exclude) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io_error(error)),
    };
    if !existing.lines().any(|line| line.trim() == "/.ait/") {
        fs::create_dir_all(
            exclude
                .parent()
                .ok_or_else(|| other("invalid Git exclude path"))?,
        )
        .map_err(io_error)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(exclude)
            .map_err(io_error)?;
        file.write_all(b"\n/.ait/\n").map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String, ControlStoreError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(other(format!(
            "cannot prepare Project storage: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|error| other(error.to_string()))
}

fn reject_symlink(path: &Path) -> Result<(), ControlStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(other(format!(
            "Project storage must not be a symlink: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

pub(super) fn apply_batch(
    transaction: &Transaction<'_>,
    batch: &ProjectBatch,
) -> Result<(), ControlStoreError> {
    for change in &batch.changes {
        apply_change(transaction, change.clone())?;
    }
    for event in &batch.events {
        transaction.execute("INSERT INTO durable_events(cursor,kind,entity_id,body_json,created_at) VALUES(?1,?2,?3,?4,?5)", params![event.cursor,event.kind,event.entity_id,event.body.to_string(),event.created_at]).map_err(sql_error)?;
    }
    for checkpoint in &batch.progress {
        transaction.execute(
            "INSERT INTO run_progress VALUES(?1, ?2, ?3) ON CONFLICT(run_id) DO UPDATE SET body_json=excluded.body_json, updated_at=excluded.updated_at",
            params![checkpoint.run_id, checkpoint.body.to_string(), checkpoint.updated_at],
        ).map_err(sql_error)?;
    }
    for run_id in &batch.clear_progress {
        transaction
            .execute("DELETE FROM run_progress WHERE run_id=?1", [run_id])
            .map_err(sql_error)?;
    }
    check_foreign_keys(transaction)
}

pub(super) fn prepare_project(
    connection: &mut Connection,
    operation_id: &str,
    batch: &ProjectBatch,
) -> Result<(), ControlStoreError> {
    // Validate the complete transaction before publishing a commit decision.
    let validation = connection.transaction().map_err(sql_error)?;
    apply_batch(&validation, batch)?;
    validation.rollback().map_err(sql_error)?;
    connection.execute(
        "INSERT INTO prepared_commit VALUES(1, ?1, ?2) ON CONFLICT(singleton) DO UPDATE SET operation_id=excluded.operation_id, body_json=excluded.body_json",
        params![operation_id, serde_json::to_string(batch).map_err(json_error)?],
    ).map_err(sql_error)?;
    Ok(())
}

pub(super) fn finish_project(
    connection: &mut Connection,
    operation_id: &str,
) -> Result<(), ControlStoreError> {
    let transaction = connection.transaction().map_err(sql_error)?;
    let last: Option<String> = transaction
        .query_row(
            "SELECT last_operation FROM project_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if last.as_deref() == Some(operation_id) {
        return Ok(());
    }
    let body: String = transaction
        .query_row(
            "SELECT body_json FROM prepared_commit WHERE singleton=1 AND operation_id=?1",
            [operation_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    apply_batch(
        &transaction,
        &serde_json::from_str(&body).map_err(json_error)?,
    )?;
    transaction
        .execute(
            "UPDATE project_identity SET last_operation=?1 WHERE singleton=1",
            [operation_id],
        )
        .map_err(sql_error)?;
    transaction
        .execute("DELETE FROM prepared_commit", [])
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}
