use super::*;

#[test]
fn locks_are_exclusive_identity_survives_and_instances_change() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("server");
    let first = InstanceLease::acquire(&directory).unwrap();
    assert!(InstanceLease::acquire(&directory).is_err());
    let (server_id, instance_id) = (first.server_id, first.instance_id);
    drop(first);
    let second = InstanceLease::acquire(&directory).unwrap();
    assert_eq!(second.server_id, server_id);
    assert_ne!(second.instance_id, instance_id);
    let independent = InstanceLease::acquire(&root.path().join("other")).unwrap();
    assert_ne!(independent.server_id, server_id);
    assert!(!root.path().join(".ait").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn corrupt_identity_is_not_overwritten_and_failed_start_releases_lock() {
    let directory = tempfile::tempdir().unwrap();
    for invalid in ["broken", "00000000-0000-0000-0000-000000000000"] {
        std::fs::write(directory.path().join("server-id"), invalid).unwrap();
        assert!(InstanceLease::acquire(directory.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("server-id")).unwrap(),
            invalid
        );
    }
    let id = Uuid::new_v4();
    std::fs::write(directory.path().join("server-id"), id.to_string()).unwrap();
    assert_eq!(
        InstanceLease::acquire(directory.path()).unwrap().server_id,
        id
    );
    assert!(InstanceLease::acquire(&directory.path().join("server-id")).is_err());
}

#[test]
#[cfg(unix)]
fn aliases_share_the_lock_and_state_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("server");
    let instance = InstanceLease::acquire(&directory).unwrap();
    let alias = root.path().join("alias");
    symlink(&directory, &alias).unwrap();
    assert!(InstanceLease::acquire(&alias).is_err());
    drop(instance);
    assert!(InstanceLease::acquire(&alias).is_ok());
    for state in ["server-id", "instance.lock"] {
        let other = tempfile::tempdir().unwrap();
        symlink(directory.join(state), other.path().join(state)).unwrap();
        assert!(InstanceLease::acquire(other.path()).is_err());
    }
}
