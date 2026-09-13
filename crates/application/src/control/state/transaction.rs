//! Typed change collection and the single record/event commit boundary.
use crate::control::runs::journal::WorkspaceRunJournal;
use ait_contracts::{
    AgentProviderView, AgentView, CronView, MessageView, ProjectView, RunView, SessionView,
    SettingsDocument,
};
use ait_ports::{
    ControlChange, ControlRecord, ControlRecordKind as Kind, ControlStore, ControlStoreError,
    PendingEvent,
};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(in crate::control) trait RecordContext: Clone + DeserializeOwned {
    const FIELDS: &'static [(&'static str, Kind)];
    fn changes(&self, updated: &Self) -> Vec<TypedChange>;
}

/// Only the context's declared record families can produce typed changes.
pub(in crate::control) enum TypedChange {
    Project(ProjectView),
    Agent(AgentView),
    Provider(AgentProviderView),
    Session(SessionView),
    Message(MessageView),
    Run(Box<RunView>),
    Cron(CronView),
    ProviderCredential(String, String),
    RunCredential(String, String),
    WorkspaceRunJournal(String, WorkspaceRunJournal),
    Settings(SettingsDocument, u64),
    Delete(Kind, String),
}

pub(in crate::control) trait Entity: Clone + PartialEq {
    const KIND: Kind;
    fn id(&self) -> &str;
    fn change(self) -> TypedChange;
}
macro_rules! entity {
    ($ty:ty, $kind:ident, $($id:ident).+) => {
        impl Entity for $ty {
            const KIND: Kind = Kind::$kind;
            fn id(&self) -> &str { &self.$($id).+ }
            fn change(self) -> TypedChange { TypedChange::$kind(self.into()) }
        }
    };
}
entity!(ProjectView, Project, id);
entity!(AgentView, Agent, id);
entity!(AgentProviderView, Provider, provider.id);
entity!(SessionView, Session, id);
entity!(MessageView, Message, id);
entity!(RunView, Run, id);
entity!(CronView, Cron, id);

pub(in crate::control) fn diff_records<T: Entity>(
    before: &[T],
    after: &[T],
    changes: &mut Vec<TypedChange>,
) {
    let before = before
        .iter()
        .map(|v| (v.id(), v))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .iter()
        .map(|v| (v.id(), v))
        .collect::<BTreeMap<_, _>>();
    for (id, value) in &after {
        if before.get(id) != Some(value) {
            changes.push((*value).clone().change());
        }
    }
    for id in before.keys() {
        if !after.contains_key(id) {
            changes.push(TypedChange::Delete(T::KIND, (*id).to_owned()));
        }
    }
}

pub(in crate::control) fn diff_map<T: Clone + PartialEq>(
    before: &HashMap<String, T>,
    after: &HashMap<String, T>,
    kind: Kind,
    put: fn(String, T) -> TypedChange,
    changes: &mut Vec<TypedChange>,
) {
    for (id, value) in after {
        if before.get(id) != Some(value) {
            changes.push(put(id.clone(), value.clone()));
        }
    }
    for id in before.keys() {
        if !after.contains_key(id) {
            changes.push(TypedChange::Delete(kind, id.clone()));
        }
    }
}

/// Baseline and provenance belong to the transaction, never to a reducer.
pub(in crate::control) struct RecordTransaction<C> {
    pub(in crate::control) revision: u64,
    pub(in crate::control) original: C,
    loaded: BTreeSet<(Kind, String)>,
    projects: BTreeMap<(Kind, String), Option<String>>,
}

impl<C: RecordContext> RecordTransaction<C> {
    pub(in crate::control) fn new(revision: u64, original: C, records: &[ControlRecord]) -> Self {
        Self {
            revision,
            original,
            loaded: records.iter().map(|r| (r.kind, r.id.clone())).collect(),
            projects: records
                .iter()
                .map(|r| ((r.kind, r.id.clone()), r.project_id.clone()))
                .collect(),
        }
    }

    pub(in crate::control) fn changes(
        &self,
        updated: &C,
    ) -> Result<Vec<ControlChange>, ControlStoreError> {
        let typed = self.original.changes(updated);
        let mut projects = self.projects.clone();
        for change in &typed {
            if let TypedChange::Run(run) = change {
                projects.insert((Kind::Run, run.id.clone()), Some(run.project_id.clone()));
            }
        }
        typed
            .into_iter()
            .filter(|change| match change {
                TypedChange::Delete(kind, id) => self.loaded.contains(&(*kind, id.clone())),
                _ => true,
            })
            .map(|change| super::codec::encode_change(change, &projects))
            .collect()
    }

    pub(in crate::control) async fn commit(
        &self,
        store: &dyn ControlStore,
        updated: &C,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        store
            .apply(self.revision, self.changes(updated)?, events)
            .await
            .map(|_| ())
    }
}
