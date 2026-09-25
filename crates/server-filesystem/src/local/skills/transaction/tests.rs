use super::*;

fn fixture() -> (tempfile::TempDir, LocalSkills) {
    let root = tempfile::tempdir().unwrap();
    let store = LocalSkills::new(
        &root.path().join("bundle"),
        &["agents", "claude", "codex"].map(|name| root.path().join(name)),
        &root.path().join("state"),
    )
    .unwrap();
    (root, store)
}

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn plan(store: &LocalSkills) -> Journal {
    let mut journal = Journal {
        id: Uuid::new_v4().to_string(),
        roots: store.targets.clone(),
        previous: store.selection().unwrap(),
        entries: vec![],
        committed: false,
    };
    let ops = store.scan(&Selection::All {}).unwrap().ops;
    prepare(store, &mut journal, &ops).unwrap();
    journal
}

#[test]
fn interrupted_update_restores_backup_and_previous_selection_idempotently() {
    for published in [false, true] {
        let (_root, mut store) = fixture();
        put(&store.source.join("alpha/SKILL.md"), "new");
        put(&store.targets[0].join("alpha/SKILL.md"), "old");
        put(&store.targets[0].join("alpha/notes"), "user");
        let previous = Selection::Custom {
            skills: vec!["alpha".into()],
        };
        store.import(&previous).unwrap();
        let journal = plan(&store);
        let stage = stage(&journal, 0, "alpha");
        fs::rename(store.targets[0].join("alpha"), stage.join("before")).unwrap();
        if published {
            fs::rename(stage.join("after"), store.targets[0].join("alpha")).unwrap();
        }
        store.import(&Selection::All {}).unwrap();
        recover(&mut store).unwrap();
        recover(&mut store).unwrap();
        assert_eq!(store.selection().unwrap(), Some(previous));
        assert_eq!(
            fs::read_to_string(store.targets[0].join("alpha/SKILL.md")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(store.targets[0].join("alpha/notes")).unwrap(),
            "user"
        );
        assert!(!store.state.join("transaction.json").exists());
    }
}

#[test]
fn interrupted_add_is_removed_but_unpublished_external_directory_is_preserved() {
    let (_root, mut store) = fixture();
    put(&store.source.join("alpha/SKILL.md"), "new");
    let journal = plan(&store);
    fs::rename(
        stage(&journal, 0, "alpha").join("after"),
        store.targets[0].join("alpha"),
    )
    .unwrap();
    put(&store.targets[1].join("alpha/external"), "keep");
    recover(&mut store).unwrap();
    assert!(!store.targets[0].join("alpha").exists());
    assert_eq!(
        fs::read_to_string(store.targets[1].join("alpha/external")).unwrap(),
        "keep"
    );
    assert_eq!(store.selection().unwrap(), None);
}

#[test]
fn external_changes_after_publication_preserve_backup_and_block_destructive_recovery() {
    let (_root, mut store) = fixture();
    put(&store.source.join("alpha/SKILL.md"), "new");
    put(&store.targets[0].join("alpha/SKILL.md"), "old");
    let journal = plan(&store);
    let stage = stage(&journal, 0, "alpha");
    fs::rename(store.targets[0].join("alpha"), stage.join("before")).unwrap();
    fs::rename(stage.join("after"), store.targets[0].join("alpha")).unwrap();
    put(&store.targets[0].join("alpha/user"), "external");
    assert_eq!(recover(&mut store), Err(ErrorCode::ResourceExhausted));
    assert_eq!(
        fs::read_to_string(stage.join("before/SKILL.md")).unwrap(),
        "old"
    );
    assert!(store.targets[0].join("alpha/user").exists());
    fs::remove_file(store.targets[0].join("alpha/user")).unwrap();
    recover(&mut store).unwrap();
}

#[test]
fn committed_journal_only_cleans_staging_and_does_not_rollback() {
    let (_root, mut store) = fixture();
    put(&store.source.join("alpha/SKILL.md"), "new");
    let mut journal = plan(&store);
    publish(
        &mut store,
        &mut journal,
        &Selection::All {},
        ApplyMode::Save,
    )
    .unwrap();
    assert!(store.state.join("transaction.json").exists());
    recover(&mut store).unwrap();
    assert_eq!(store.scan(&Selection::All {}).unwrap().state, "up-to-date");
    assert_eq!(store.selection().unwrap(), Some(Selection::All {}));
}

#[test]
fn changed_plan_before_publish_rolls_back_without_overwriting_external_files() {
    let (_root, mut store) = fixture();
    put(&store.source.join("alpha/SKILL.md"), "new");
    put(&store.targets[0].join("alpha/SKILL.md"), "old");
    let mut journal = plan(&store);
    put(&store.targets[0].join("alpha/SKILL.md"), "external");
    assert_eq!(
        publish(
            &mut store,
            &mut journal,
            &Selection::All {},
            ApplyMode::Save
        ),
        Err(ErrorCode::ResourceExhausted)
    );
    recover(&mut store).unwrap();
    assert_eq!(
        fs::read_to_string(store.targets[0].join("alpha/SKILL.md")).unwrap(),
        "external"
    );
}

#[test]
fn invalid_journal_paths_and_roots_are_rejected() {
    let (_root, mut store) = fixture();
    put(&store.source.join("alpha/SKILL.md"), "new");
    let mut journal = plan(&store);
    journal.entries[0].name = "../outside".into();
    tree::save(&store.state.join("transaction.json"), &journal).unwrap();
    assert_eq!(recover(&mut store), Err(ErrorCode::RegistryIo));
    journal.entries[0].name = "alpha".into();
    journal.id = "../outside".into();
    tree::save(&store.state.join("transaction.json"), &journal).unwrap();
    assert_eq!(recover(&mut store), Err(ErrorCode::RegistryIo));
}

#[test]
fn interrupted_deletion_restores_the_entire_directory() {
    let (_root, mut store) = fixture();
    put(&store.targets[0].join("paseo-chat/nested/user"), "preserve");
    let journal = plan(&store);
    fs::rename(
        store.targets[0].join("paseo-chat"),
        stage(&journal, 0, "paseo-chat").join("before"),
    )
    .unwrap();
    recover(&mut store).unwrap();
    assert_eq!(
        fs::read_to_string(store.targets[0].join("paseo-chat/nested/user")).unwrap(),
        "preserve"
    );
}

#[test]
fn interrupted_preparation_is_cleaned_without_live_mutations() {
    let (_root, mut store) = fixture();
    let journal = Journal {
        id: Uuid::new_v4().to_string(),
        roots: store.targets.clone(),
        previous: None,
        entries: vec![],
        committed: false,
    };
    tree::save(&store.state.join("transaction.json"), &journal).unwrap();
    let stage = stage(&journal, 0, "alpha");
    put(&stage.join("after/SKILL.md"), "partial");
    recover(&mut store).unwrap();
    assert!(!stage.exists());
    assert!(!store.targets[0].join("alpha").exists());
}

#[test]
fn new_removals_during_publish_prevent_commit_and_restore_previous_targets() {
    let (_root, mut store) = fixture();
    put(&store.source.join("alpha/SKILL.md"), "new");
    put(&store.targets[0].join("alpha/SKILL.md"), "old");
    let mut journal = plan(&store);
    put(&store.targets[1].join("paseo-chat/user"), "external");
    assert_eq!(
        publish(
            &mut store,
            &mut journal,
            &Selection::All {},
            ApplyMode::Save
        ),
        Err(ErrorCode::ResourceExhausted)
    );
    recover(&mut store).unwrap();
    assert_eq!(
        fs::read_to_string(store.targets[0].join("alpha/SKILL.md")).unwrap(),
        "old"
    );
    assert!(store.targets[1].join("paseo-chat/user").exists());
    assert_eq!(store.selection().unwrap(), None);
}
