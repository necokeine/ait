use super::*;

#[cfg(unix)]
#[tokio::test]
async fn unconfirmed_worker_blocks_execution_until_verified_host_restart() {
    use std::os::unix::process::CommandExt;
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let id = format!("worker-{}", temp.path().display());
    let first = PortableSqliteControlStore::open(temp.path().join("first")).unwrap();
    create(&first, &root, &id).await;
    let owner = first
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version
        .projects[&id]
        .owner(&id);
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    first
        .register_worker_process(&owner, child.id())
        .await
        .unwrap();
    let blocked = first.close_project(&id).await.unwrap_err().to_string();
    drop(first); // Models the OS releasing daemon locks while its child survives.
    let second = PortableSqliteControlStore::open(temp.path().join("second")).unwrap();
    let takeover = second
        .open_project(root.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let read = second
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    assert!(
        second
            .apply_versioned(&read.version, vec![], vec![])
            .await
            .is_err()
    );
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(blocked.contains("RECOVERY_BLOCKED"));
    assert!(
        takeover.value["execution_blocked"]
            .as_str()
            .unwrap()
            .contains("RECOVERY_BLOCKED")
    );
    assert!(
        second
            .apply_versioned(&read.version, vec![], vec![])
            .await
            .is_err()
    );
    // Simulate a verified boot generation change; PID disappearance alone is insufficient.
    second
        .access(|state| {
            state.projects[&id]
                .connection
                .execute(
                    "UPDATE worker_processes SET boot_id='previous-host-boot'",
                    [],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    let current = second
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    second
        .apply_versioned(&current.version, vec![], vec![])
        .await
        .unwrap();
    assert!(
        second
            .release_worker_process(&owner, child.id())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn copied_directory_cannot_run_with_the_same_project_identity() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("one");
    let copy = temp.path().join("two");
    std::fs::create_dir_all(copy.join(".ait")).unwrap();
    let id = format!("copy-{}", temp.path().display());
    let first = PortableSqliteControlStore::open(temp.path().join("first")).unwrap();
    create(&first, &root, &id).await;
    first
        .access(|state| {
            state.projects[&id]
                .connection
                .backup(crate::MAIN_DB, copy.join(".ait/project.sqlite3"), None)
                .unwrap();
            Ok(())
        })
        .unwrap();
    let second = PortableSqliteControlStore::open(temp.path().join("second")).unwrap();
    assert!(
        second
            .open_project(copy.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string()
            .contains("PROJECT_BUSY")
    );
    first.close_project(&id).await.unwrap();
    assert_eq!(
        second
            .open_project(copy.to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
}
