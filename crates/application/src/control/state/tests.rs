//! Faults at record planning and commit boundaries.
#![allow(clippy::pedantic)]
use super::codec::decode_records;
use super::commands::CommandTransaction;
use super::records::RecordAccess;
use super::*;
use ait_contracts::{Command, CommandResult};
use ait_ports::*;
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

mod project_preparation;

const ROOT: &str = "00000000-0000-4000-8000-000000000001";
const NEXT: &str = "00000000-0000-4000-8000-000000000002";

struct Probe {
    inner: SqliteControlStore,
    scripted: Mutex<VecDeque<ControlRead>>,
    reads: Mutex<Vec<Vec<ControlFilter>>>,
    conflict_once: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    apply_attempts: AtomicUsize,
}
impl Probe {
    fn new(scripted: Vec<ControlRead>) -> Arc<Self> {
        Arc::new(Self {
            inner: SqliteControlStore::in_memory().unwrap(),
            scripted: Mutex::new(scripted.into()),
            reads: Mutex::new(Vec::new()),
            conflict_once: Mutex::new(None),
            apply_attempts: AtomicUsize::new(0),
        })
    }
    fn access(self: &Arc<Self>) -> RecordAccess {
        RecordAccess {
            store: self.clone(),
        }
    }
}
#[async_trait]
impl ControlStore for Probe {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.reads.lock().unwrap().push(filters.to_vec());
        let scripted = self.scripted.lock().unwrap().pop_front();
        if let Some(read) = scripted {
            Ok(read)
        } else {
            self.inner.read(filters).await
        }
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        self.apply_attempts.fetch_add(1, Ordering::SeqCst);
        let fault = self.conflict_once.lock().unwrap().take();
        if let Some(fault) = fault {
            fault();
            return Err(ControlStoreError::Conflict);
        }
        self.inner.apply(revision, changes, events).await
    }
    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.inner.replay(cursor, limit).await
    }
    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }
    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.inner.replay_page(cursor, limit).await
    }
    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.inner.save_progress(checkpoint, events).await
    }
    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress(project_id).await
    }
    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
}

fn record(kind: Kind, id: &str, value: Value) -> ControlRecord {
    ControlRecord {
        kind,
        id: id.into(),
        project_id: match kind {
            Kind::Project
            | Kind::Session
            | Kind::Message
            | Kind::Run
            | Kind::WorkspaceRunJournal => Some("p".into()),
            _ => None,
        },
        value,
    }
}
fn read(revision: u64, records: Vec<ControlRecord>) -> ControlRead {
    ControlRead { revision, records }
}
fn project(root: &str) -> ControlRecord {
    record(
        Kind::Project,
        "p",
        json!({"id":"p","name":"Project","workdir":"/project","root_message_id":root,"base_commit":"a".repeat(40)}),
    )
}
fn agent() -> ControlRecord {
    record(
        Kind::Agent,
        "a",
        json!({"id":"a","name":"Agent","config":{"provider_id":"builtin-codex","model":"test"},"revision":1,"enabled":true}),
    )
}
fn session(head: &str) -> ControlRecord {
    record(
        Kind::Session,
        "s",
        json!({"id":"s","project_id":"p","agent_id":"a","current_message_id":head,"active_run_id":null,"version":1}),
    )
}
fn message(id: &str, parent: Option<&str>) -> ControlRecord {
    record(
        Kind::Message,
        id,
        json!({"id":id,"project_id":"p","parent_message_id":parent,"role":if parent.is_none(){"system"}else{"assistant"},"kind":"standard","text":"text"}),
    )
}
fn run(head: &str) -> ControlRecord {
    record(
        Kind::Run,
        "r",
        json!({"id":"r","project_id":"p","base_message_id":ROOT,"last_message_id":head,"session_id":"s","agent_id":"a","agent_revision":1,"config":{"provider_id":"builtin-codex","model":"test"},"provider":builtin_providers()[0].provider,"trigger":"manual","cron_id":null,"scheduled_at":null,"status":"running","error":null}),
    )
}

#[test]
fn synthetic_defaults_never_authorize_deletes() {
    let empty = decode_records::<ProviderContext>(&read(1, vec![])).unwrap();
    let mut updated = empty.original.clone();
    updated.providers.clear();
    assert!(empty.changes(&updated).unwrap().is_empty());
    let provider = record(
        Kind::Provider,
        "builtin-codex",
        serde_json::to_value(&builtin_providers()[0]).unwrap(),
    );
    let loaded = decode_records::<ProviderContext>(&read(1, vec![provider])).unwrap();
    let mut updated = loaded.original.clone();
    updated.providers.clear();
    assert_eq!(
        loaded.changes(&updated).unwrap(),
        vec![ControlChange::Delete {
            kind: Kind::Provider,
            id: "builtin-codex".into()
        }]
    );
}

#[test]
fn hydrated_unchanged_records_are_not_reencoded_or_rewritten() {
    let loaded = decode_records::<ArchiveContext>(&read(
        1,
        vec![project(ROOT), session(ROOT), message(ROOT, None)],
    ))
    .unwrap();
    assert_eq!(loaded.original.sessions[0].workdir, "/project/.ait/s");
    assert!(loaded.changes(&loaded.original).unwrap().is_empty());
    let mut updated = loaded.original.clone();
    updated.sessions[0].name = "Renamed".into();
    let changes = loaded.changes(&updated).unwrap();
    assert!(
        matches!(changes.as_slice(),[ControlChange::Put(r)] if r.kind==Kind::Session && r.id=="s")
    );
}

#[tokio::test]
async fn duplicate_transaction_conflicts_without_duplicate_events() {
    let store = SqliteControlStore::in_memory().unwrap();
    let loaded = decode_records::<SettingsContext>(
        &store
            .read(&[ControlFilter::id(Kind::Settings, "settings")])
            .await
            .unwrap(),
    )
    .unwrap();
    let mut updated = loaded.original.clone();
    updated.settings_revision += 1;
    let event = PendingEvent {
        kind: "settings.saved".into(),
        entity_id: None,
        body: json!({}),
        created_at: 1,
    };
    loaded
        .commit(&store, &updated, vec![event.clone()])
        .await
        .unwrap();
    assert_eq!(
        loaded.commit(&store, &updated, vec![event]).await,
        Err(ControlStoreError::Conflict)
    );
    assert_eq!(store.replay(0, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn session_reference_revision_is_checked_before_decoding() {
    let store = Probe::new(vec![
        read(1, vec![session(ROOT)]),
        read(1, vec![agent()]),
        read(
            2,
            vec![record(
                Kind::Session,
                "s",
                json!({"corrupt":"stale response"}),
            )],
        ),
        read(2, vec![session(NEXT)]),
        read(2, vec![agent()]),
        read(
            2,
            vec![
                session(NEXT),
                agent(),
                project(ROOT),
                message(NEXT, Some(ROOT)),
            ],
        ),
    ]);
    let loaded = store
        .access()
        .read_command_records(&Command::SendMessage {
            session_id: "s".into(),
            text: "hello".into(),
        })
        .await
        .unwrap();
    let CommandTransaction::Conversation(loaded) = loaded else {
        panic!("typed context")
    };
    assert_eq!(loaded.revision, 2);
    assert_eq!(loaded.original.sessions[0].current_message_id(), NEXT);
    let reads = store.reads.lock().unwrap();
    assert!(reads[2].contains(&ControlFilter::id(Kind::Message, ROOT)));
    assert!(reads[5].contains(&ControlFilter::id(Kind::Message, NEXT)));
    assert!(
        !reads
            .iter()
            .flatten()
            .any(|f| matches!(f, ControlFilter::All(_) | ControlFilter::Project { .. }))
    );
}

#[tokio::test]
async fn new_session_replans_changed_root_and_reads_only_ancestors() {
    let store = Probe::new(vec![
        read(1, vec![project(ROOT), agent()]),
        read(2, vec![record(Kind::Message, ROOT, json!(null))]),
        read(2, vec![project(NEXT), agent()]),
        read(2, vec![project(NEXT), agent(), message(NEXT, None)]),
    ]);
    let loaded = store
        .access()
        .read_command_records(&Command::CreateSession {
            id: "new".into(),
            project_id: "p".into(),
            agent_id: "a".into(),
            at_message_id: None,
        })
        .await
        .unwrap();
    let CommandTransaction::NewSession(loaded) = loaded else {
        panic!("typed context")
    };
    assert_eq!(loaded.original.messages[0].id, NEXT);
    let reads = store.reads.lock().unwrap();
    assert!(reads[1].contains(&ControlFilter::message_ancestors(ROOT)));
    assert!(reads[3].contains(&ControlFilter::message_ancestors(NEXT)));
    assert!(reads.iter().flatten().all(|f| matches!(
        f,
        ControlFilter::Id {
            kind: Kind::Project | Kind::Agent | Kind::Session,
            ..
        } | ControlFilter::MessageAncestors { .. }
    )));
}

#[tokio::test]
async fn export_retries_revision_and_keeps_the_complete_message_family() {
    let first = vec![project(ROOT), session(ROOT), message(ROOT, None)];
    let mut complete = first.clone();
    complete.push(message(NEXT, Some(ROOT)));
    let mut final_records = complete.clone();
    final_records.push(agent());
    let store = Probe::new(vec![
        read(1, first),
        read(2, vec![]),
        read(2, complete),
        read(2, vec![agent()]),
        read(2, final_records),
    ]);
    let command = Command::ExportProject {
        project_id: "p".into(),
    };
    let result = store
        .access()
        .read_command_records(&command)
        .await
        .unwrap()
        .read(command)
        .unwrap();
    let CommandResult::ProjectExport(archive) = result else {
        panic!("export")
    };
    assert_eq!(archive.source_revision, 2);
    assert_eq!(archive.messages.len(), 2);
    assert!(
        store
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|filters| filters.contains(&ControlFilter::project(Kind::Message, "p")))
    );
}

#[tokio::test]
async fn api_run_plan_reselects_head_without_catalog_settings_or_journal() {
    let store = Probe::new(vec![
        read(1, vec![run(ROOT)]),
        read(2, vec![record(Kind::Run, "r", json!(null))]),
        read(2, vec![run(NEXT)]),
        read(
            2,
            vec![
                run(NEXT),
                session(NEXT),
                message(ROOT, None),
                message(NEXT, Some(ROOT)),
            ],
        ),
    ]);
    let tx = store.access().read_api_run_records("r").await.unwrap();
    assert_eq!(tx.original.runs[0].last_message_id().as_deref(), Some(NEXT));
    let reads = store.reads.lock().unwrap();
    assert!(reads[3].contains(&ControlFilter::message_ancestors(NEXT)));
    assert!(reads.iter().flatten().all(|f| matches!(
        f,
        ControlFilter::Id {
            kind: Kind::Run | Kind::Session,
            ..
        } | ControlFilter::MessageAncestors { .. }
    )));
}

#[tokio::test]
async fn unrelated_corrupt_records_do_not_block_typed_session_commit() {
    let store = Probe::new(vec![]);
    store
        .inner
        .apply(
            0,
            vec![
                ControlChange::Put(project(ROOT)),
                ControlChange::Put(message(ROOT, None)),
                ControlChange::Put(session(ROOT)),
                ControlChange::Put(run(ROOT)),
                ControlChange::Put(record(Kind::Agent, "a", json!({"broken":true}))),
                ControlChange::Put(record(Kind::Settings, "settings", json!({"broken":true}))),
                ControlChange::Put(record(
                    Kind::WorkspaceRunJournal,
                    "r",
                    json!({"broken":true}),
                )),
            ],
            vec![],
        )
        .await
        .unwrap();
    let service = crate::control::LocalControlService::new(
        Arc::new(ait_project_local::LocalProjectWorkspace::default()),
        store.clone(),
    );
    let result = service
        .execute(Command::RenameSession {
            session_id: "s".into(),
            name: "Renamed".into(),
        })
        .await;
    assert!(result.ok, "{:?}", result.error);
    assert!(
        store
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|filters| filters == &[ControlFilter::id(Kind::Session, "s")])
    );
    assert_eq!(
        store
            .inner
            .read(&[ControlFilter::id(Kind::Agent, "a")])
            .await
            .unwrap()
            .records[0]
            .value,
        json!({"broken":true})
    );
    // The runtime mutation context never opens the corrupt catalog or journal.
    let run = store.access().read_api_run_records("r").await.unwrap();
    assert_eq!(run.original.runs[0].id, "r");
}

#[tokio::test]
async fn get_run_reads_only_run_even_with_corrupt_related_records() {
    let store = Probe::new(vec![]);
    store
        .inner
        .apply(
            0,
            vec![
                ControlChange::Put(run(ROOT)),
                ControlChange::Put(record(Kind::Project, "p", json!({"broken":true}))),
                ControlChange::Put(record(Kind::Session, "s", json!({"broken":true}))),
                ControlChange::Put(record(
                    Kind::WorkspaceRunJournal,
                    "r",
                    json!({"broken":true}),
                )),
            ],
            vec![],
        )
        .await
        .unwrap();
    let command = Command::GetRun { run_id: "r".into() };
    let tx = store.access().read_command_records(&command).await.unwrap();
    assert!(matches!(tx, CommandTransaction::Runs(_)));
    let CommandResult::Run(found) = tx.read(command.clone()).unwrap() else {
        panic!("expected Run")
    };
    assert_eq!(found.id, "r");
    let service = crate::control::LocalControlService::new(
        Arc::new(ait_project_local::LocalProjectWorkspace::default()),
        store.clone(),
    );
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(
        *store.reads.lock().unwrap(),
        vec![
            vec![ControlFilter::id(Kind::Run, "r")],
            vec![ControlFilter::id(Kind::Run, "r")],
        ]
    );
    assert_eq!(store.apply_attempts.load(Ordering::SeqCst), 0);
}

#[test]
fn codec_rejects_an_undeclared_family_or_mismatched_record_identity() {
    assert!(decode_records::<SessionsContext>(&read(1, vec![agent()])).is_err());
    let mut wrong = session(ROOT);
    wrong.id = "another-session".into();
    assert!(decode_records::<SessionsContext>(&read(1, vec![wrong])).is_err());
}

#[tokio::test]
async fn rebinding_reads_the_selected_agent_without_decoding_the_old_agent() {
    let mut replacement = agent();
    replacement.id = "replacement".into();
    replacement.value["id"] = json!("replacement");
    let store = Probe::new(vec![
        read(1, vec![session(ROOT)]),
        read(1, vec![session(ROOT), replacement]),
    ]);
    let tx = store
        .access()
        .read_command_records(&Command::SetSessionAgent {
            session_id: "s".into(),
            agent_id: "replacement".into(),
        })
        .await
        .unwrap();
    assert!(matches!(tx, CommandTransaction::SessionBinding(_)));
    assert_eq!(
        store.reads.lock().unwrap()[1],
        vec![
            ControlFilter::id(Kind::Session, "s"),
            ControlFilter::id(Kind::Agent, "replacement")
        ]
    );
}

#[tokio::test]
async fn storage_rejects_message_rewrite_and_rolls_back_pointer_and_event_together() {
    let store = SqliteControlStore::in_memory().unwrap();
    store
        .apply(
            0,
            vec![
                ControlChange::Put(project(ROOT)),
                ControlChange::Put(message(ROOT, None)),
                ControlChange::Put(session(ROOT)),
            ],
            vec![],
        )
        .await
        .unwrap();
    let loaded = decode_records::<ApiRunContext>(
        &store
            .read(&[
                ControlFilter::id(Kind::Session, "s"),
                ControlFilter::id(Kind::Message, ROOT),
            ])
            .await
            .unwrap(),
    )
    .unwrap();
    let mut updated = loaded.original.clone();
    updated.messages[0].text = Some("rewrite".into());
    updated.sessions[0].name = "must roll back".into();
    let event = PendingEvent {
        kind: "session.updated".into(),
        entity_id: Some("s".into()),
        body: json!({}),
        created_at: 1,
    };
    assert!(loaded.commit(&store, &updated, vec![event]).await.is_err());
    let after = store
        .read(&[ControlFilter::id(Kind::Session, "s")])
        .await
        .unwrap();
    assert_eq!(after.revision, loaded.revision);
    assert_ne!(after.records[0].value["name"], "must roll back");
    assert!(store.replay(0, 10).await.unwrap().is_empty());
}
