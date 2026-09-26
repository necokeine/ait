//! Paseo orchestration-skills operations/sync selection behavior.

use super::*;

#[test]
fn missing_bundle_is_a_read_only_empty_catalog_and_keeps_personal_skills() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("agents/personal/SKILL.md"), "personal");
    let mut service = Skills::new(Box::new(store(root.path())));
    let status = call(&mut service, skills::GET_STATUS, json!({}));
    assert_eq!(status["state"], "not-installed");
    assert_eq!(status["available"], json!([]));
    assert_eq!(status["installed"], json!([]));
    assert!(!root.path().join("state").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("agents/personal/SKILL.md")).unwrap(),
        "personal"
    );
}

#[test]
fn clean_custom_selection_installs_only_selected_skills_in_every_target() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "alpha");
    put(&root.path().join("bundle/beta/SKILL.md"), "beta");
    put(
        &root.path().join("bundle/alpha/references/nested/readme.md"),
        "reference",
    );
    let mut service = Skills::new(Box::new(store(root.path())));
    let result = call(
        &mut service,
        skills::SAVE_SELECTION,
        json!({"selection":{"mode":"custom","skills":["alpha"]}}),
    );
    assert_eq!(result["state"], "up-to-date");
    assert_eq!(result["available"], json!(["alpha", "beta"]));
    assert_eq!(result["installed"], json!(["alpha"]));
    for target in ["agents", "claude", "codex"] {
        assert_eq!(
            fs::read_to_string(
                root.path()
                    .join(target)
                    .join("alpha/references/nested/readme.md")
            )
            .unwrap(),
            "reference"
        );
        assert!(!root.path().join(target).join("beta").exists());
    }
}

#[test]
fn mixed_add_update_delete_operations_are_sorted_by_skill_name() {
    let root = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta"] {
        put(
            &root.path().join("bundle").join(name).join("SKILL.md"),
            "current",
        );
    }
    for target in ["agents", "claude", "codex"] {
        put(&root.path().join(target).join("beta/SKILL.md"), "old");
        put(
            &root.path().join(target).join("paseo-chat/SKILL.md"),
            "legacy",
        );
    }
    let mut service = Skills::new(Box::new(store(root.path())));
    let status = call(&mut service, skills::GET_STATUS, json!({}));
    assert_eq!(status["state"], "drift");
    assert_eq!(
        status["ops"],
        json!([
            {"kind":"add","name":"alpha"},
            {"kind":"update","name":"beta"},
            {"kind":"delete","name":"paseo-chat"}
        ])
    );
}

#[test]
fn no_op_resync_keeps_installed_file_revision_and_user_extras() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "current");
    let mut service = Skills::new(Box::new(store(root.path())));
    call(&mut service, skills::RECONCILE, json!({}));
    let path = root.path().join("agents/alpha/SKILL.md");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    put(&root.path().join("agents/alpha/personal"), "keep");
    let result = call(&mut service, skills::RECONCILE, json!({}));
    assert_eq!(result["state"], "up-to-date");
    assert_eq!(result["ops"], json!([]));
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
    assert_eq!(
        fs::read_to_string(root.path().join("agents/alpha/personal")).unwrap(),
        "keep"
    );
}

#[test]
fn all_selection_picks_up_a_new_bundle_skill_on_the_next_reconcile() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "alpha");
    let mut service = Skills::new(Box::new(store(root.path())));
    call(&mut service, skills::RECONCILE, json!({}));
    put(&root.path().join("bundle/beta/SKILL.md"), "beta");
    let pending = call(&mut service, skills::GET_STATUS, json!({}));
    assert_eq!(pending["ops"], json!([{"kind":"add","name":"beta"}]));
    assert_eq!(
        call(&mut service, skills::RECONCILE, json!({}))["installed"],
        json!(["alpha", "beta"])
    );
    assert_eq!(
        fs::read_to_string(root.path().join("codex/beta/SKILL.md")).unwrap(),
        "beta"
    );
}

#[test]
fn unshipped_custom_names_are_preserved_until_the_bundle_ships_them() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "alpha");
    let mut service = Skills::new(Box::new(store(root.path())));
    let selection = json!({"mode":"custom","skills":["future"]});
    let result = call(
        &mut service,
        skills::SAVE_SELECTION,
        json!({"selection":selection}),
    );
    assert_eq!(result["selection"], selection);
    assert_eq!(result["installed"], json!([]));
    assert_eq!(result["ops"], json!([]));
    put(&root.path().join("bundle/future/SKILL.md"), "future");
    let result = call(&mut service, skills::RECONCILE, json!({}));
    assert_eq!(result["installed"], json!(["future"]));
    assert!(!root.path().join("agents/alpha").exists());
}

#[test]
fn incomplete_deletion_confirmation_keeps_all_targets_and_previous_selection() {
    let root = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta"] {
        put(
            &root.path().join("bundle").join(name).join("SKILL.md"),
            name,
        );
    }
    let mut service = Skills::new(Box::new(store(root.path())));
    call(&mut service, skills::RECONCILE, json!({}));
    let result = call(
        &mut service,
        skills::SAVE_SELECTION,
        json!({
            "selection":{"mode":"custom","skills":[]},"confirmedRemovals":["alpha"]
        }),
    );
    assert_eq!(
        result["confirmationRequired"],
        json!({"removals":["alpha", "beta"]})
    );
    assert_eq!(result["selection"], json!({"mode":"all"}));
    for target in ["agents", "claude", "codex"] {
        for name in ["alpha", "beta"] {
            assert_eq!(
                fs::read_to_string(root.path().join(target).join(name).join("SKILL.md")).unwrap(),
                name
            );
        }
    }
    assert!(!root.path().join("state/selection.json").exists());
}

#[test]
fn removed_bundle_skill_is_left_on_disk_as_an_unmanaged_directory() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "original");
    let mut service = Skills::new(Box::new(store(root.path())));
    call(&mut service, skills::RECONCILE, json!({}));
    fs::remove_dir_all(root.path().join("bundle/alpha")).unwrap();
    call(&mut service, skills::RECONCILE, json!({}));
    call(&mut service, skills::UNINSTALL, json!({}));
    for target in ["agents", "claude", "codex"] {
        assert_eq!(
            fs::read_to_string(root.path().join(target).join("alpha/SKILL.md")).unwrap(),
            "original"
        );
    }
}

#[test]
fn uninstall_is_idempotent_with_missing_targets_and_unrelated_user_skills() {
    let root = tempfile::tempdir().unwrap();
    put(&root.path().join("bundle/alpha/SKILL.md"), "alpha");
    put(&root.path().join("claude/paseo-chat/SKILL.md"), "legacy");
    put(&root.path().join("codex/personal/SKILL.md"), "personal");
    let mut service = Skills::new(Box::new(store(root.path())));
    assert_eq!(
        call(&mut service, skills::UNINSTALL, json!({}))["state"],
        "not-installed"
    );
    assert_eq!(
        call(&mut service, skills::UNINSTALL, json!({}))["state"],
        "not-installed"
    );
    assert!(!root.path().join("claude/paseo-chat").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("codex/personal/SKILL.md")).unwrap(),
        "personal"
    );
}
