//! Typed persistence contexts owned by Run use cases.
use std::collections::HashMap;

use ait_contracts::SettingsDocument;

use crate::control::conversation::{MessageRecord, SessionRecord};
use crate::control::persistence::define_record_context;
use crate::control::project::ProjectRecord;
use crate::control::runs::RunRecord;
use crate::control::runs::journal::WorkspaceRunJournal;

define_record_context!(RunsContext {
    runs: Vec<RunRecord>,
} [ "runs" => Run ]);

define_record_context!(RunContext {
    projects: Vec<ProjectRecord>,
    sessions: Vec<SessionRecord>,
    messages: Vec<MessageRecord>,
    runs: Vec<RunRecord>,
    workspace_run_journals: HashMap<String, WorkspaceRunJournal>,
    run_credentials: HashMap<String, String>,
    settings: SettingsDocument,
    settings_revision: u64,
} [
    "projects" => Project,
    "sessions" => Session,
    "messages" => Message,
    "runs" => Run,
    "workspace_run_journals" => WorkspaceRunJournal,
    "run_credentials" => RunCredential,
    "settings" => Settings
]);

define_record_context!(RunControlContext {
    projects: Vec<ProjectRecord>,
    sessions: Vec<SessionRecord>,
    runs: Vec<RunRecord>,
    workspace_run_journals: HashMap<String, WorkspaceRunJournal>,
} [
    "projects" => Project,
    "sessions" => Session,
    "runs" => Run,
    "workspace_run_journals" => WorkspaceRunJournal
]);

define_record_context!(ApiRunContext {
    runs: Vec<RunRecord>,
    sessions: Vec<SessionRecord>,
    messages: Vec<MessageRecord>,
} [ "runs" => Run, "sessions" => Session, "messages" => Message ]);
