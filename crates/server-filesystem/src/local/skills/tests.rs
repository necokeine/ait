use super::*;
use crate::protocol::skills::{self, Selection};
use crate::service::skills::Skills;
use serde_json::{Value, json};

mod paseo;

fn store(root: &std::path::Path) -> LocalSkills {
    LocalSkills::new(
        &root.join("bundle"),
        &["agents", "claude", "codex"].map(|name| root.join(name)),
        &root.join("state"),
    )
    .unwrap()
}

fn put(path: &std::path::Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn call(service: &mut Skills, method: &str, params: Value) -> Value {
    service.execute(method, params).unwrap()
}

#[test]
fn install_repairs_all_targets_preserves_extras_and_requires_deletion_consent() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    put(&root.join("bundle/alpha/SKILL.md"), "one");
    put(&root.join("bundle/beta/SKILL.md"), "two");
    put(&root.join("agents/personal/note"), "user");
    let mut service = Skills::new(Box::new(store(root)));
    let initial = call(&mut service, skills::GET_STATUS, json!({}));
    assert_eq!(initial["state"], "not-installed");
    assert_eq!(initial["available"], json!(["alpha", "beta"]));
    assert_eq!(
        initial["ops"],
        json!([{"kind":"add","name":"alpha"},{"kind":"add","name":"beta"}])
    );
    assert!(!root.join("state").exists());
    let installed = call(&mut service, skills::RECONCILE, json!({}));
    assert_eq!(installed["state"], "up-to-date");
    for target in ["agents", "claude", "codex"] {
        put(&root.join(target).join("alpha/notes/personal.txt"), "keep");
        assert_eq!(
            fs::read_to_string(root.join(target).join("beta/SKILL.md")).unwrap(),
            "two"
        );
    }
    assert_eq!(
        call(&mut service, skills::GET_STATUS, json!({}))["state"],
        "up-to-date"
    );
    put(&root.join("codex/alpha/SKILL.md"), "drift");
    put(&root.join("agents/paseo-chat/extra"), "legacy");
    assert_eq!(
        call(&mut service, skills::GET_STATUS, json!({}))["ops"],
        json!([{"kind":"update","name":"alpha"},{"kind":"delete","name":"paseo-chat"}])
    );
    call(&mut service, skills::RECONCILE, json!({}));
    assert!(root.join("agents/paseo-chat/extra").exists());
    let request =
        json!({"selection":{"mode":"custom","skills":[" alpha ", "unknown", "alpha", ""]}});
    let pending = call(&mut service, skills::SAVE_SELECTION, request.clone());
    assert_eq!(pending["selection"], json!({"mode":"all"}));
    assert_eq!(
        pending["confirmationRequired"],
        json!({"removals":["beta","paseo-chat"]})
    );
    assert!(!root.join("state/selection.json").exists());
    let mut confirmed = request;
    confirmed["confirmedRemovals"] = json!(["beta", "paseo-chat"]);
    let saved = call(&mut service, skills::SAVE_SELECTION, confirmed);
    assert!(saved["confirmationRequired"].is_null());
    assert_eq!(
        saved["selection"],
        json!({"mode":"custom","skills":["alpha","unknown"]})
    );
    for target in ["agents", "claude", "codex"] {
        assert_eq!(
            fs::read_to_string(root.join(target).join("alpha/notes/personal.txt")).unwrap(),
            "keep"
        );
        assert!(!root.join(target).join("beta").exists());
    }
    assert!(root.join("agents/personal/note").exists());
    drop(service);
    let mut service = Skills::new(Box::new(store(root)));
    assert_eq!(
        call(&mut service, skills::GET_STATUS, json!({}))["selection"],
        saved["selection"]
    );
    let removed = call(&mut service, skills::UNINSTALL, json!({}));
    assert_eq!(removed["state"], "not-installed");
    assert_eq!(removed["selection"], saved["selection"]);
    assert!(root.join("agents/personal/note").exists());
    assert_eq!(
        call(&mut service, skills::RECONCILE, json!({}))["installed"],
        json!(["alpha"])
    );
    assert!(!root.join("state/transaction.json").exists());
}

#[test]
fn managed_file_manifest_prunes_only_unmodified_obsolete_files() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    for name in ["SKILL.md", "obsolete", "modified"] {
        put(&root.join("bundle/alpha").join(name), "v1");
    }
    let mut service = Skills::new(Box::new(store(root)));
    call(&mut service, skills::RECONCILE, json!({}));
    put(&root.join("agents/alpha/modified"), "user changed");
    put(&root.join("agents/alpha/extra"), "personal");
    fs::remove_file(root.join("bundle/alpha/obsolete")).unwrap();
    fs::remove_file(root.join("bundle/alpha/modified")).unwrap();
    put(&root.join("bundle/alpha/SKILL.md"), "v2");
    call(&mut service, skills::RECONCILE, json!({}));
    assert!(!root.join("agents/alpha/obsolete").exists());
    assert_eq!(
        fs::read_to_string(root.join("agents/alpha/modified")).unwrap(),
        "user changed"
    );
    assert!(root.join("agents/alpha/extra").exists());
    assert!(!root.join("codex/alpha/modified").exists());
    assert_eq!(
        call(&mut service, skills::RECONCILE, json!({}))["state"],
        "up-to-date"
    );
}

#[test]
fn legacy_import_is_normalized_once_and_does_not_install() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "v1");
    let mut service = Skills::new(Box::new(store(root.path())));
    let first = call(
        &mut service,
        skills::IMPORT_LEGACY_SELECTION,
        json!({"selection":{"mode":"custom","skills":["z", " alpha ", "alpha", ""]}}),
    );
    assert_eq!(
        first,
        json!({"imported":true,"selection":{"mode":"custom","skills":["alpha","z"]}})
    );
    assert!(!root.path().join("agents").exists());
    drop(service);
    let mut service = Skills::new(Box::new(store(root.path())));
    assert_eq!(
        call(
            &mut service,
            skills::IMPORT_LEGACY_SELECTION,
            json!({"selection":{"mode":"all"}})
        )["imported"],
        false
    );
    let empty = call(
        &mut service,
        skills::SAVE_SELECTION,
        json!({"selection":{"mode":"custom","skills":[]}}),
    );
    assert_eq!(empty["state"], "not-installed");
    assert_eq!(empty["ops"], json!([]));
    assert_eq!(
        call(
            &mut service,
            skills::SAVE_SELECTION,
            json!({"selection":{"mode":"all"}})
        )["installed"],
        json!(["alpha"])
    );
}

#[test]
fn missing_target_takes_precedence_and_stale_plans_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "v1");
    put(&root.path().join("agents/alpha/SKILL.md"), "edited");
    let mut store = store(root.path());
    let snapshot = store.scan(&Selection::All {}).unwrap();
    assert_eq!(snapshot.installed, ["alpha"]);
    assert_eq!(snapshot.ops[0].kind, Kind::Add);
    assert_eq!(
        store.apply(&Selection::All {}, &[], ApplyMode::Save),
        Err(ErrorCode::ResourceExhausted)
    );
    store
        .apply(&Selection::All {}, &snapshot.ops, ApplyMode::Save)
        .unwrap();
    assert_eq!(store.scan(&Selection::All {}).unwrap().state, "up-to-date");
}

#[test]
fn overlapping_roots_bad_paths_and_corrupt_selection_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        LocalSkills::new(
            &root.path().join("bundle"),
            &["bundle/nested", "claude", "codex"].map(|name| root.path().join(name)),
            &root.path().join("state")
        )
        .is_err()
    );
    assert!(tree::normalize(std::path::Path::new("relative")).is_err());
    assert!(!tree::valid_relative("../secret"));
    assert!(!tree::valid_relative("bad\\path"));
    put(&root.path().join("state/selection.json"), "broken");
    assert_eq!(store(root.path()).selection(), Err(ErrorCode::RegistryIo));
}

#[cfg(unix)]
#[test]
fn symlinks_and_hostile_manifest_are_rejected_without_mutating_targets() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    put(&root.join("bundle/alpha/SKILL.md"), "new");
    put(&root.join("agents/alpha/SKILL.md"), "old");
    put(
        &root.join("agents/alpha/.paseo-managed-files.json"),
        r#"{"version":1,"files":{"../outside":"abc"}}"#,
    );
    let mut service = Skills::new(Box::new(store(root)));
    assert!(service.execute(skills::RECONCILE, json!({})).is_err());
    assert_eq!(
        fs::read_to_string(root.join("agents/alpha/SKILL.md")).unwrap(),
        "old"
    );
    assert!(fs::read_dir(root.join("agents")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ait-skills-")
    }));
    fs::remove_file(root.join("agents/alpha/.paseo-managed-files.json")).unwrap();
    std::os::unix::fs::symlink(
        root.join("bundle/alpha/SKILL.md"),
        root.join("agents/alpha/link"),
    )
    .unwrap();
    assert!(service.execute(skills::GET_STATUS, json!({})).is_err());
}

#[cfg(unix)]
#[test]
fn updates_preserve_user_file_and_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    put(&root.join("bundle/alpha/SKILL.md"), "new");
    put(&root.join("agents/alpha/SKILL.md"), "old");
    put(&root.join("agents/alpha/private/run.sh"), "personal");
    fs::set_permissions(
        root.join("agents/alpha/private/run.sh"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::set_permissions(
        root.join("agents/alpha/private"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::set_permissions(root.join("agents/alpha"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut service = Skills::new(Box::new(store(root)));
    call(&mut service, skills::RECONCILE, json!({}));
    for name in [
        "agents/alpha",
        "agents/alpha/private",
        "agents/alpha/private/run.sh",
    ] {
        assert_eq!(
            fs::metadata(root.join(name)).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
