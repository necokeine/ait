//! File-backed counterparts of Paseo schedule/store.test.ts.
use super::*;
use crate::{engine::Engine, engine::tests::input, ports::Outcome};
use serde_json::json;

fn time() -> chrono::DateTime<chrono::Utc> {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

fn path(root: &tempfile::TempDir) -> PathBuf {
    root.path()
        .canonicalize()
        .unwrap()
        .join("schedules/data.json")
}

fn create(engine: &mut Engine) -> String {
    let mut params = input();
    params["runOnCreate"] = json!(false);
    engine
        .request("schedule.create.request", params, time())
        .unwrap()["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn real_store_roundtrips_updated_configuration_and_occurrence_history() {
    let root = tempfile::tempdir().unwrap();
    let file = path(&root);
    let mut engine = Engine::open(Box::new(FileStore::new(file.clone())), time()).unwrap();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, true, time()).unwrap();
    engine
        .finish(
            &id,
            &run,
            true,
            Outcome {
                agent_id: Some("agent".into()),
                workspace_id: Some("workspace".into()),
                output: Some("persisted answer".into()),
                ..Outcome::default()
            },
            time(),
        )
        .unwrap();
    let updated = engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,
        "name":"renamed","prompt":"next prompt","maxRuns":3,
        "cadence":{"type":"cron","expression":"0 9 * * *","timezone":"Asia/Shanghai"}}),
            time(),
        )
        .unwrap();
    drop(engine);
    let loaded = Engine::open(Box::new(FileStore::new(file)), time()).unwrap();
    assert_eq!(json!(loaded.inspect(&id).unwrap()), updated["schedule"]);
    assert_eq!(loaded.inspect(&id).unwrap().runs.len(), 1);
}

#[test]
fn deleting_one_schedule_removes_it_from_disk_without_changing_its_sibling() {
    let root = tempfile::tempdir().unwrap();
    let file = path(&root);
    let mut engine = Engine::open(Box::new(FileStore::new(file.clone())), time()).unwrap();
    let first = create(&mut engine);
    let second = create(&mut engine);
    let sibling = json!(engine.inspect(&second).unwrap());
    engine
        .request(
            "schedule.delete.request",
            json!({"scheduleId":first}),
            time(),
        )
        .unwrap();
    let reloaded = FileStore::new(file).load().unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(json!(reloaded[0]), sibling);
}

#[test]
fn oversized_atomic_replacement_keeps_the_previous_valid_document() {
    let root = tempfile::tempdir().unwrap();
    let file = path(&root);
    let mut engine = Engine::open(Box::new(FileStore::new(file.clone())), time()).unwrap();
    let id = create(&mut engine);
    let previous = fs::read(&file).unwrap();
    let mut record = engine.inspect(&id).unwrap();
    record.prompt = "x".repeat(16 * 1024 * 1024);
    let mut store = FileStore::new(file.clone());
    assert_eq!(store.save(&[record]).unwrap_err(), Error::Conflict);
    assert_eq!(fs::read(&file).unwrap(), previous);
    assert_eq!(store.load().unwrap().len(), 1);
    assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn oversized_or_truncated_state_is_rejected_without_rewriting_source_bytes() {
    let root = tempfile::tempdir().unwrap();
    let file = path(&root);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    fs::write(&file, &oversized).unwrap();
    let store = FileStore::new(file.clone());
    assert_eq!(store.load().unwrap_err(), Error::Storage);
    assert_eq!(fs::metadata(&file).unwrap().len(), oversized.len() as u64);
    let truncated = br#"{"version":1,"schedules":["#;
    fs::write(&file, truncated).unwrap();
    assert_eq!(store.load().unwrap_err(), Error::Storage);
    assert_eq!(fs::read(file).unwrap(), truncated);
}

#[cfg(unix)]
#[test]
fn symlinked_parent_cannot_redirect_schedule_reads_or_writes() {
    let root = tempfile::tempdir().unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let actual = canonical.join("actual");
    fs::create_dir(&actual).unwrap();
    let file = actual.join("data.json");
    let mut original = FileStore::new(file.clone());
    original.save(&[]).unwrap();
    std::os::unix::fs::symlink(&actual, canonical.join("linked")).unwrap();
    let mut redirected = FileStore::new(canonical.join("linked/data.json"));
    assert_eq!(redirected.load().unwrap_err(), Error::Storage);
    assert_eq!(redirected.save(&[]).unwrap_err(), Error::Storage);
    assert!(original.load().unwrap().is_empty());
}

#[test]
fn failed_temporary_file_creation_does_not_leave_partial_state() {
    let root = tempfile::tempdir().unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let obstruction = canonical.join("schedules");
    fs::write(&obstruction, "existing file").unwrap();
    let mut store = FileStore::new(obstruction.join("data.json"));
    assert_eq!(store.save(&[]).unwrap_err(), Error::Storage);
    assert_eq!(fs::read_to_string(obstruction).unwrap(), "existing file");
    assert_eq!(fs::read_dir(canonical).unwrap().count(), 1);
}
