use super::*;

#[test]
fn versions_compare_dependencies_and_ownership_without_equating_catalogs() {
    let mut first = ControlVersion::legacy(3);
    first.catalog_id = "catalog".into();
    let mut second = first.clone();
    second.projects.insert(
        "project".into(),
        ProjectVersion {
            runtime_instance_id: "runtime".into(),
            owner_epoch: 1,
            revision: 2,
        },
    );
    assert!(first.compatible_with(&second));
    first = second.clone();
    second.projects.get_mut("project").unwrap().owner_epoch += 1;
    assert!(!first.compatible_with(&second));
    second = first.clone();
    second.catalog_id = "another".into();
    assert!(!first.compatible_with(&second));
    assert!(!ControlVersion::legacy(1).compatible_with(&ControlVersion::legacy(2)));
}
