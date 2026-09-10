#![allow(dead_code)]
#![allow(missing_docs)]
#![allow(clippy::pedantic)]

use std::collections::{BTreeMap, HashMap};

use ait_application::LocalControlService;
use ait_contracts::{
    AgentProviderView, AgentView, Command, CommandResult, CronView, MessageView, ProjectView,
    RunView, SessionView, default_settings,
};
use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlRecord, ControlRecordKind, ControlStore,
    ControlStoreError, PendingEvent,
};
use async_trait::async_trait;
use serde_json::{Value, json};

const KINDS: [ControlRecordKind; 11] = [
    ControlRecordKind::Project,
    ControlRecordKind::Agent,
    ControlRecordKind::Provider,
    ControlRecordKind::ProviderCredential,
    ControlRecordKind::RunCredential,
    ControlRecordKind::Session,
    ControlRecordKind::Message,
    ControlRecordKind::Run,
    ControlRecordKind::WorkspaceRunJournal,
    ControlRecordKind::Cron,
    ControlRecordKind::Settings,
];

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceView {
    pub projects: Vec<ProjectView>,
    pub agents: Vec<AgentView>,
    pub providers: Vec<AgentProviderView>,
    pub sessions: Vec<SessionView>,
    pub messages: Vec<MessageView>,
    pub runs: Vec<RunView>,
    pub crons: Vec<CronView>,
}

async fn execute(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.expect("successful result")
}

/// Test-only aggregate assembled from bounded list commands.
pub async fn workspace(service: &LocalControlService) -> WorkspaceView {
    let CommandResult::Projects(projects) = execute(service, Command::ListProjects).await else {
        panic!("expected projects")
    };
    let CommandResult::Agents(agents) = execute(service, Command::ListAgents).await else {
        panic!("expected agents")
    };
    let CommandResult::AgentProviders(providers) =
        execute(service, Command::ListAgentProviders).await
    else {
        panic!("expected providers")
    };
    let CommandResult::Crons(crons) = execute(service, Command::ListCrons).await else {
        panic!("expected crons")
    };
    let mut sessions = Vec::new();
    let mut messages = Vec::new();
    let mut runs = Vec::new();
    for project in &projects {
        let CommandResult::Sessions(mut project_sessions) = execute(
            service,
            Command::ListSessions {
                project_id: project.id.clone(),
            },
        )
        .await
        else {
            panic!("expected sessions")
        };
        sessions.append(&mut project_sessions);
        let CommandResult::Messages(mut project_messages) = execute(
            service,
            Command::ListMessages {
                project_id: project.id.clone(),
            },
        )
        .await
        else {
            panic!("expected messages")
        };
        messages.append(&mut project_messages);
        let CommandResult::Runs(mut project_runs) = execute(
            service,
            Command::ListRuns {
                project_id: project.id.clone(),
            },
        )
        .await
        else {
            panic!("expected runs")
        };
        runs.append(&mut project_runs);
    }
    WorkspaceView {
        projects,
        agents,
        providers,
        sessions,
        messages,
        runs,
        crons,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestState {
    pub revision: u64,
    pub value: Value,
}

#[async_trait]
pub trait ControlStoreTestExt: ControlStore {
    async fn load(&self) -> Result<TestState, ControlStoreError> {
        self.load_state().await
    }

    async fn load_state(&self) -> Result<TestState, ControlStoreError> {
        let filters = KINDS.map(ControlFilter::all);
        let read = self.read(&filters).await?;
        Ok(TestState {
            revision: read.revision,
            value: decode(read),
        })
    }

    async fn replace_state(
        &self,
        expected_revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<TestState, ControlStoreError> {
        let before = self.read(&KINDS.map(ControlFilter::all)).await?;
        let before = before
            .records
            .into_iter()
            .map(|record| ((record.kind, record.id.clone()), record))
            .collect::<BTreeMap<_, _>>();
        let after = encode(&value);
        let mut changes = Vec::new();
        for (key, record) in &after {
            if before.get(key) != Some(record) {
                changes.push(ControlChange::Put(record.clone()));
            }
        }
        for ((kind, id), _) in before {
            if !after.contains_key(&(kind, id.clone())) {
                changes.push(ControlChange::Delete { kind, id });
            }
        }
        let revision = self.apply(expected_revision, changes, events).await?;
        Ok(TestState { revision, value })
    }

    async fn commit(
        &self,
        expected_revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<TestState, ControlStoreError> {
        self.replace_state(expected_revision, value, events).await
    }
}

impl<T: ControlStore + ?Sized> ControlStoreTestExt for T {}

pub fn terminal_run_status(changes: &[ControlChange]) -> Option<&str> {
    changes.iter().rev().find_map(|change| match change {
        ControlChange::Put(record) if record.kind == ControlRecordKind::Run => {
            record.value["status"].as_str()
        }
        _ => None,
    })
}

fn decode(read: ControlRead) -> Value {
    let mut value = json!({
        "projects": [], "agents": [], "providers": [],
        "provider_credentials": {}, "run_credentials": {},
        "sessions": [], "messages": [], "runs": [],
        "workspace_run_journals": {}, "crons": [],
        "settings": default_settings(), "settings_revision": 1,
    });
    for record in read.records {
        match record.kind {
            ControlRecordKind::Project => {
                value["projects"].as_array_mut().unwrap().push(record.value)
            }
            ControlRecordKind::Agent => value["agents"].as_array_mut().unwrap().push(record.value),
            ControlRecordKind::Provider => value["providers"]
                .as_array_mut()
                .unwrap()
                .push(record.value),
            ControlRecordKind::ProviderCredential => {
                value["provider_credentials"][record.id] = record.value
            }
            ControlRecordKind::RunCredential => value["run_credentials"][record.id] = record.value,
            ControlRecordKind::Session => {
                value["sessions"].as_array_mut().unwrap().push(record.value)
            }
            ControlRecordKind::Message => {
                value["messages"].as_array_mut().unwrap().push(record.value)
            }
            ControlRecordKind::Run => value["runs"].as_array_mut().unwrap().push(record.value),
            ControlRecordKind::WorkspaceRunJournal => {
                value["workspace_run_journals"][record.id] = record.value
            }
            ControlRecordKind::Cron => value["crons"].as_array_mut().unwrap().push(record.value),
            ControlRecordKind::Settings => {
                value["settings"] = record.value["values"].clone();
                value["settings_revision"] = record.value["revision"].clone();
            }
        }
    }
    value
}

fn encode(value: &Value) -> BTreeMap<(ControlRecordKind, String), ControlRecord> {
    let mut records = BTreeMap::new();
    let mut put = |kind, id: String, project_id: Option<String>, value: Value| {
        records.insert(
            (kind, id.clone()),
            ControlRecord {
                kind,
                id,
                project_id,
                value,
            },
        );
    };
    for (name, kind, project_scoped) in [
        ("projects", ControlRecordKind::Project, true),
        ("agents", ControlRecordKind::Agent, false),
        ("providers", ControlRecordKind::Provider, false),
        ("sessions", ControlRecordKind::Session, true),
        ("messages", ControlRecordKind::Message, true),
        ("runs", ControlRecordKind::Run, true),
        ("crons", ControlRecordKind::Cron, true),
    ] {
        for item in value[name].as_array().into_iter().flatten() {
            let id = item["id"].as_str().expect("record id").to_owned();
            let project_id = project_scoped.then(|| {
                if kind == ControlRecordKind::Project {
                    id.clone()
                } else {
                    item["project_id"].as_str().expect("project id").to_owned()
                }
            });
            put(kind, id, project_id, item.clone());
        }
    }
    let run_projects = value["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|run| {
            Some((
                run["id"].as_str()?.to_owned(),
                run["project_id"].as_str()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    for (name, kind) in [
        (
            "provider_credentials",
            ControlRecordKind::ProviderCredential,
        ),
        ("run_credentials", ControlRecordKind::RunCredential),
        (
            "workspace_run_journals",
            ControlRecordKind::WorkspaceRunJournal,
        ),
    ] {
        for (id, item) in value[name].as_object().into_iter().flatten() {
            let project_id = (kind != ControlRecordKind::ProviderCredential)
                .then(|| run_projects.get(id).cloned())
                .flatten();
            put(kind, id.clone(), project_id, item.clone());
        }
    }
    put(
        ControlRecordKind::Settings,
        "settings".into(),
        None,
        json!({
            "values": value.get("settings").cloned().unwrap_or_else(|| json!(default_settings())),
            "revision": value.get("settings_revision").cloned().unwrap_or_else(|| json!(1)),
        }),
    );
    records
}
