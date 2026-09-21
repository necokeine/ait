use super::*;
use crate::LocalWorkspace;
use server_domain::ProjectId;
use server_ports::Workspace;

#[test]
fn epochs_survive_release_and_never_regress_to_a_copied_database() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = LocalWorkspace::new(directory.path().join("locks"));
    let id = ProjectId::generate();
    let mut lease = workspace.acquire_identity(id).unwrap();
    assert_eq!(
        lease
            .reserve_epoch(OwnerEpoch::new(5).unwrap())
            .unwrap()
            .value(),
        6
    );
    drop(lease);
    let mut next = workspace.acquire_identity(id).unwrap();
    assert_eq!(
        next.reserve_epoch(OwnerEpoch::new(1).unwrap())
            .unwrap()
            .value(),
        7
    );
    assert_eq!(
        next.reserve_epoch(OwnerEpoch::new(50).unwrap())
            .unwrap()
            .value(),
        51
    );
    let counter = directory.path().join(format!("locks/{id}.epoch"));
    for corrupt in ["", "corrupt", "18446744073709551615", "9223372036854775807"] {
        std::fs::write(&counter, corrupt).unwrap();
        assert_eq!(
            next.reserve_epoch(OwnerEpoch::new(1).unwrap()),
            Err(ProjectError::Invalid)
        );
        assert_eq!(std::fs::read_to_string(&counter).unwrap(), corrupt);
    }
}

#[cfg(unix)]
#[test]
fn counter_symlinks_are_rejected_without_replacing_the_target() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = LocalWorkspace::new(directory.path().join("locks"));
    let id = ProjectId::generate();
    let mut lease = workspace.acquire_identity(id).unwrap();
    let external = directory.path().join("external");
    std::fs::write(&external, "preserve").unwrap();
    std::os::unix::fs::symlink(
        &external,
        directory.path().join(format!("locks/{id}.epoch")),
    )
    .unwrap();
    assert!(lease.reserve_epoch(OwnerEpoch::new(0).unwrap()).is_err());
    assert_eq!(std::fs::read_to_string(external).unwrap(), "preserve");
}
