//! Deterministic projection of authoritative Codex Thread history.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use ait_contracts::{AgentMode, ApiError, CodexThreadView, CommandResult};
use ait_domain::{
    AgentId, CodexThreadSource, CodexWorkspaceMode, CodexWriterState, DomainError, DomainMetadata,
    ErrorCode, Message, MessageId, MessageKind, MessageOrigin, MessageRole, ProjectId,
    ProviderHistoryCompleteness, ProviderItem, ProviderRelationshipState, ProviderSyncState,
    SessionId, SessionReference, SessionSource, SessionStatus, SubMessage, TimestampMs,
};
use ait_ports::{
    CodexItemsView, CodexThreadSnapshot, CodexThreadSourceKind, CodexTurnSnapshot, ControlFilter,
    ControlRecordKind, ControlStoreError,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::control::{
    LocalControlService,
    conversation::{CodexImportContext, MessageRecord, SessionRecord},
    errors::{error, project_error, store_error},
    events::pending,
};

const PROJECTION_VERSION: u32 = 1;
const MAX_PROVIDER_STRING_CHARS: usize = 20_000;
const MAX_PROVIDER_ARRAY_ITEMS: usize = 256;
const MAX_PROVIDER_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_PROVIDER_DEPTH: usize = 16;

/// Input identities fixed while materializing one native Thread.
#[derive(Clone, Copy)]
#[allow(
    clippy::struct_field_names,
    reason = "provider, Project, and Agent identities must remain explicit at this boundary"
)]
pub(in crate::control) struct CodexImportIdentity<'a> {
    pub(in crate::control) provider_id: &'a str,
    pub(in crate::control) project_id: &'a str,
    pub(in crate::control) agent_id: &'a str,
}

/// Idempotent immutable records and Session ref produced by one complete read.
pub(in crate::control) struct CodexMaterialization {
    pub(in crate::control) session: SessionRecord,
    pub(in crate::control) messages: Vec<MessageRecord>,
}

const ALL_SOURCE_KINDS: &[CodexThreadSourceKind] = &[
    CodexThreadSourceKind::Cli,
    CodexThreadSourceKind::Vscode,
    CodexThreadSourceKind::Exec,
    CodexThreadSourceKind::AppServer,
    CodexThreadSourceKind::SubAgent,
    CodexThreadSourceKind::SubAgentReview,
    CodexThreadSourceKind::SubAgentCompact,
    CodexThreadSourceKind::SubAgentThreadSpawn,
    CodexThreadSourceKind::SubAgentOther,
    CodexThreadSourceKind::Unknown,
];

impl LocalControlService {
    pub(in crate::control) async fn list_codex_threads(
        &self,
        provider_id: &str,
        project_id: Option<&str>,
    ) -> Result<CommandResult, ApiError> {
        let catalog = self
            .records()
            .read_records::<CodexImportContext>(vec![
                ControlFilter::id(ControlRecordKind::Provider, provider_id),
                ControlFilter::all(ControlRecordKind::Session),
                ControlFilter::all(ControlRecordKind::Project),
            ])
            .await?;
        validate_codex_provider(&catalog.original, provider_id)?;
        if project_id.is_some_and(|id| {
            !catalog
                .original
                .projects
                .iter()
                .any(|project| project.id == id)
        }) {
            return Err(error(ErrorCode::InvalidProject, "Project not found", false));
        }
        let bindings = catalog
            .original
            .sessions
            .iter()
            .filter_map(|session| {
                if let SessionSource::CodexThread(source) = &session.source
                    && source.provider_id == provider_id
                {
                    Some((
                        source.thread_id.as_str(),
                        (session.id.clone(), session.project_id.clone()),
                    ))
                } else {
                    None
                }
            })
            .collect::<HashMap<_, _>>();
        let mut owners_by_cwd = HashMap::new();
        let source = self.require_codex_history_source()?;
        let mut threads = source
            .list_threads(ALL_SOURCE_KINDS)
            .await
            .map_err(|failure| history_api_error(ErrorCode::CodexHistoryListFailed, failure))?
            .into_iter()
            .filter(|thread| {
                let Some(project_id) = project_id else {
                    return true;
                };
                if let Some((_, bound_project)) = bindings.get(thread.id.as_str()) {
                    return bound_project == project_id;
                }
                let owners = owners_by_cwd.entry(thread.cwd.clone()).or_insert_with(|| {
                    matching_projects(&catalog.original, Path::new(&thread.cwd))
                });
                owners.len() == 1 && owners.contains(project_id)
            })
            .map(|thread| {
                let (session_id, project_id) = bindings
                    .get(thread.id.as_str())
                    .map(|(session, project)| (Some(session.clone()), Some(project.clone())))
                    .unwrap_or_default();
                CodexThreadView {
                    provider_id: provider_id.to_owned(),
                    thread_id: thread.id,
                    codex_session_id: thread.session_id,
                    forked_from_thread_id: thread.forked_from_id,
                    cwd: thread.cwd,
                    name: thread.name.map(|value| bounded_provider_string(&value)),
                    preview: bounded_provider_string(&thread.preview),
                    source: sanitize_provider_value(&thread.source, 0),
                    status: sanitize_provider_value(&thread.status, 0),
                    archived: thread.archived,
                    created_at: thread.created_at,
                    updated_at: thread.updated_at,
                    native_metadata: sanitize_provider_value(&Value::Object(thread.metadata), 0),
                    session_id,
                    project_id,
                }
            })
            .collect::<Vec<_>>();
        threads.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.thread_id.cmp(&right.thread_id))
        });
        Ok(CommandResult::CodexThreads(threads))
    }

    pub(in crate::control) async fn sync_codex_thread(
        &self,
        provider_id: &str,
        thread_id: &str,
        project_id: &str,
        agent_id: &str,
    ) -> Result<CommandResult, ApiError> {
        for _ in 0..4 {
            let loaded = self
                .records()
                .read_records::<CodexImportContext>(vec![
                    ControlFilter::all(ControlRecordKind::Project),
                    ControlFilter::id(ControlRecordKind::Agent, agent_id),
                    ControlFilter::id(ControlRecordKind::Provider, provider_id),
                    ControlFilter::all(ControlRecordKind::Session),
                    ControlFilter::project(ControlRecordKind::Message, project_id),
                    ControlFilter::project(ControlRecordKind::Run, project_id),
                ])
                .await?;
            let snapshot = self
                .require_codex_history_source()?
                .read_thread(thread_id)
                .await
                .map_err(|failure| history_api_error(ErrorCode::CodexHistoryReadFailed, failure))?;
            validate_import_binding(
                &loaded.original,
                &snapshot,
                provider_id,
                project_id,
                agent_id,
            )?;
            let mut materialization = materialize_thread(
                &snapshot,
                CodexImportIdentity {
                    provider_id,
                    project_id,
                    agent_id,
                },
                &loaded.original.sessions,
                &loaded.original.messages,
            )
            .map_err(project_error)?;
            let session_view = materialization.session.view();
            let mut updated = loaded.original.clone();
            crate::control::runs::native::attribute_inputs(
                &snapshot,
                &mut materialization.messages,
                &mut updated.runs,
            )?;
            updated.messages.extend(materialization.messages);
            if let Some(index) = updated
                .sessions
                .iter()
                .position(|session| session.id == materialization.session.id)
            {
                updated.sessions[index] = materialization.session;
            } else {
                updated.sessions.push(materialization.session);
            }
            let mut events = vec![pending(
                "codex.history.synced",
                Some(session_view.id.clone()),
                &json!({
                    "provider_id": provider_id,
                    "thread_id": thread_id,
                    "project_id": project_id,
                    "session_id": session_view.id,
                }),
            )];
            events.extend(crate::control::runs::native::run_updates(
                &loaded.original.runs,
                &updated.runs,
            ));
            match self.persist_records(&loaded, &updated, events).await {
                Ok(()) => return Ok(CommandResult::Session(session_view)),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(error(
            ErrorCode::CodexHistoryReconcileConflict,
            "concurrent Codex history synchronization did not settle",
            true,
        ))
    }

    fn require_codex_history_source(
        &self,
    ) -> Result<&std::sync::Arc<dyn ait_ports::CodexHistorySource>, ApiError> {
        self.codex_history_source.as_ref().ok_or_else(|| {
            error(
                ErrorCode::CodexThreadCapabilityUnsupported,
                "Codex history access is not configured on this host",
                false,
            )
        })
    }
}

fn history_api_error(code: ErrorCode, failure: DomainError) -> ApiError {
    if matches!(
        failure.code,
        ErrorCode::CodexHistorySchemaUnsupported
            | ErrorCode::CodexHistoryIncomplete
            | ErrorCode::CodexThreadIdConflict
            | ErrorCode::CodexTurnIdConflict
            | ErrorCode::CodexHistoryCursorRepeated
    ) {
        return project_error(failure);
    }
    error(code, failure.message, failure.retryable)
}

fn validate_import_binding(
    state: &CodexImportContext,
    snapshot: &CodexThreadSnapshot,
    provider_id: &str,
    project_id: &str,
    agent_id: &str,
) -> Result<(), ApiError> {
    validate_codex_provider(state, provider_id)?;
    let agent = state
        .agents
        .iter()
        .find(|agent| agent.id == agent_id && agent.enabled)
        .ok_or_else(|| error(ErrorCode::AgentNotFound, "enabled Agent not found", false))?;
    if agent.config.provider_id != provider_id {
        return Err(error(
            ErrorCode::CodexThreadBindingConflict,
            "Codex Session Agent must use the selected Provider",
            false,
        ));
    }
    if let Some(existing) = state.sessions.iter().find(|session| {
        matches!(
            &session.source,
            SessionSource::CodexThread(source)
                if source.provider_id == provider_id && source.thread_id == snapshot.id
        )
    }) {
        if existing.project_id != project_id || existing.agent_id() != agent_id {
            return Err(error(
                ErrorCode::CodexThreadBindingConflict,
                "Codex Thread is already bound to another Project or Agent",
                false,
            ));
        }
        return Ok(());
    }
    let project_ids = matching_projects(state, Path::new(&snapshot.cwd));
    if project_ids.is_empty() {
        return Err(error(
            ErrorCode::CodexThreadProjectUnbound,
            "Codex Thread cwd does not belong to a registered Project",
            false,
        ));
    }
    if project_ids.len() > 1 {
        return Err(error(
            ErrorCode::CodexThreadProjectAmbiguous,
            "Codex Thread cwd belongs to more than one registered Project",
            false,
        ));
    }
    if !project_ids.contains(project_id) {
        return Err(error(
            ErrorCode::CodexThreadBindingConflict,
            "selected Project does not own the Codex Thread cwd",
            false,
        ));
    }
    Ok(())
}

fn validate_codex_provider(state: &CodexImportContext, provider_id: &str) -> Result<(), ApiError> {
    let provider = state
        .providers
        .iter()
        .find(|provider| provider.provider.id == provider_id)
        .ok_or_else(|| {
            error(
                ErrorCode::InvalidAgentConfiguration,
                "Codex Provider is not registered",
                false,
            )
        })?;
    if provider.provider.kind != AgentMode::Codex {
        return Err(error(
            ErrorCode::InvalidAgentConfiguration,
            "history import requires a Codex Provider",
            false,
        ));
    }
    Ok(())
}

fn matching_projects(state: &CodexImportContext, cwd: &Path) -> HashSet<String> {
    let Ok(canonical_cwd) = std::fs::canonicalize(cwd) else {
        return HashSet::new();
    };
    let mut projects = state
        .projects
        .iter()
        .filter_map(|project| {
            let root = Path::new(&project.workdir);
            let root = std::fs::canonicalize(root).ok()?;
            canonical_cwd.starts_with(root).then(|| project.id.clone())
        })
        .collect::<HashSet<_>>();
    projects.extend(state.sessions.iter().filter_map(|session| {
        let workdir = Path::new(&session.workdir);
        let workdir = std::fs::canonicalize(workdir).ok()?;
        (workdir == canonical_cwd).then(|| session.project_id.clone())
    }));
    projects
}

struct ProjectedTurn {
    snapshot: CodexTurnSnapshot,
    items: Vec<Value>,
    content_hash: String,
}

struct Segment {
    role: MessageRole,
    items: Vec<(usize, Value)>,
}

/// Converts a complete provider snapshot into immutable Messages and one Session ref.
///
/// Existing records are used only to preserve the Session identity, verify a same-Project
/// fork prefix, and reject immutable identity conflicts.
///
/// # Errors
///
/// Returns a stable Codex history error for malformed identities, incomplete non-tail
/// history, conflicting bindings, or invalid provider items.
#[allow(
    clippy::too_many_lines,
    reason = "Projection identity and Session reconciliation form one deterministic boundary"
)]
pub(in crate::control) fn materialize_thread(
    snapshot: &CodexThreadSnapshot,
    identity: CodexImportIdentity<'_>,
    existing_sessions: &[SessionRecord],
    existing_messages: &[MessageRecord],
) -> Result<CodexMaterialization, DomainError> {
    validate_snapshot_identity(snapshot, &identity)?;
    let existing_session = bound_session(
        existing_sessions,
        identity.provider_id,
        &snapshot.id,
        identity.project_id,
    )?;
    let (projected_turns, completeness) = normalize_turns(
        snapshot.turns.clone(),
        snapshot.writer_confirmed,
        existing_messages,
    )?;
    let (lineage_id, relationship_state) = resolve_lineage(
        snapshot,
        identity.provider_id,
        identity.project_id,
        existing_session,
        existing_sessions,
        existing_messages,
        &projected_turns,
    );
    let session_id = existing_session.map_or_else(
        || {
            SessionId::new(
                stable_uuid(&["session", identity.provider_id, &snapshot.id]).to_string(),
            )
        },
        |session| SessionId::new(&session.id),
    );
    let mut messages = project_messages(
        snapshot,
        &identity,
        &session_id,
        &lineage_id,
        &projected_turns,
    )?;
    let head = messages
        .last()
        .map(|message| MessageId::parse(&message.id))
        .transpose()
        .map_err(|_| history_error("projected Codex Message identity is invalid"))?
        .ok_or_else(|| history_error("Codex history projection produced no root"))?;
    reuse_existing_projection(existing_messages, &mut messages)?;
    messages.retain(|candidate| {
        !existing_messages
            .iter()
            .any(|existing| existing.id == candidate.id)
    });
    let native_cwd = PathBuf::from(&snapshot.cwd);
    let source = SessionSource::CodexThread(Box::new(CodexThreadSource {
        provider_id: identity.provider_id.to_owned(),
        thread_id: snapshot.id.clone(),
        codex_session_id: snapshot.session_id.clone(),
        lineage_id,
        forked_from_thread_id: snapshot.forked_from_id.clone(),
        relationship_state,
        source: bound_provider_payload(&snapshot.source)?,
        history_mode: (!snapshot.history_mode.is_empty()).then(|| snapshot.history_mode.clone()),
        archived: snapshot.archived,
        native_cwd: native_cwd.clone(),
        native_project_id: snapshot.project_id.clone(),
        native_status: snapshot
            .status
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        native_metadata: bound_provider_payload(&Value::Object(snapshot.metadata.clone()))?,
        writer_state: CodexWriterState::Unknown,
        workspace_mode: CodexWorkspaceMode::NativeCwd {
            cwd: native_cwd.clone(),
        },
        sync_state: if completeness == ProviderHistoryCompleteness::Full {
            ProviderSyncState::Synced
        } else {
            ProviderSyncState::Incomplete
        },
        history_completeness: completeness,
    }));
    let reference = if let Some(existing) = existing_session {
        let mut reference = existing.reference.clone();
        reference.reconcile(reference.head(), reference.version(), head)?;
        reference
    } else {
        SessionReference::new(head, AgentId::new(identity.agent_id))
    };
    let session = SessionRecord {
        reference,
        id: session_id.as_str().to_owned(),
        project_id: identity.project_id.to_owned(),
        workdir: snapshot.cwd.clone(),
        source,
        name: existing_session.map_or_else(String::new, |session| session.name.clone()),
        title: existing_session
            .and_then(|session| session.title.clone())
            .or_else(|| snapshot.name.as_deref().map(bounded_provider_string)),
        description: existing_session.map_or_else(
            || bounded_provider_string(&snapshot.preview),
            |session| session.description.clone(),
        ),
        title_generation_started: false,
        status: existing_session.map_or(SessionStatus::Active, |session| session.status),
    };
    Ok(CodexMaterialization { session, messages })
}

fn validate_snapshot_identity(
    snapshot: &CodexThreadSnapshot,
    identity: &CodexImportIdentity<'_>,
) -> Result<(), DomainError> {
    let cwd = PathBuf::from(&snapshot.cwd);
    if snapshot.id.trim().is_empty()
        || snapshot.session_id.trim().is_empty()
        || identity.provider_id.trim().is_empty()
        || identity.project_id.trim().is_empty()
        || identity.agent_id.trim().is_empty()
        || !cwd.is_absolute()
    {
        return Err(history_error(
            "Codex Thread identity, binding, or native cwd is invalid",
        ));
    }
    Ok(())
}

fn bound_session<'a>(
    sessions: &'a [SessionRecord],
    provider_id: &str,
    thread_id: &str,
    project_id: &str,
) -> Result<Option<&'a SessionRecord>, DomainError> {
    let matches = sessions
        .iter()
        .filter(|session| {
            matches!(
                &session.source,
                SessionSource::CodexThread(source)
                    if source.provider_id == provider_id && source.thread_id == thread_id
            )
        })
        .collect::<Vec<_>>();
    if matches.len() > 1
        || matches
            .first()
            .is_some_and(|session| session.project_id != project_id)
    {
        return Err(DomainError::invariant(
            ErrorCode::CodexThreadBindingConflict,
            "Codex Thread is already bound to a different Session or Project",
        ));
    }
    Ok(matches.first().copied())
}

fn normalize_turns(
    turns: Vec<CodexTurnSnapshot>,
    writer_confirmed: bool,
    existing_messages: &[MessageRecord],
) -> Result<(Vec<ProjectedTurn>, ProviderHistoryCompleteness), DomainError> {
    let confirmed: HashSet<&str> = existing_messages
        .iter()
        .filter_map(|message| {
            message
                .data
                .as_ref()
                .and_then(|data| data.pointer("/native_message/metadata/codex/turn_content_hash"))
                .and_then(Value::as_str)
        })
        .collect();
    let mut projected = Vec::with_capacity(turns.len());
    let mut incomplete = false;
    for turn in turns {
        let items = turn
            .items
            .iter()
            .map(|item| sanitize_provider_value(item, 0))
            .collect::<Vec<_>>();
        let content_hash = turn_content_hash(&turn, &items)?;
        let terminal = matches!(turn.status.as_str(), "completed" | "failed")
            || turn.status == "interrupted"
                && (turn.completed_at.is_some()
                    || writer_confirmed
                    || confirmed.contains(content_hash.as_str()));
        if turn.items_view != CodexItemsView::Full || !terminal {
            incomplete = true;
            continue;
        }
        if incomplete {
            return Err(DomainError::invariant(
                ErrorCode::CodexHistoryIncomplete,
                "Codex history contains a publishable Turn after an incomplete tail",
            ));
        }
        validate_item_identities(&items)?;
        projected.push(ProjectedTurn {
            snapshot: turn,
            items,
            content_hash,
        });
    }
    Ok((
        projected,
        if incomplete {
            ProviderHistoryCompleteness::Partial
        } else {
            ProviderHistoryCompleteness::Full
        },
    ))
}

fn validate_item_identities(items: &[Value]) -> Result<(), DomainError> {
    let mut ids = std::collections::HashSet::with_capacity(items.len());
    if items.iter().any(|item| {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        id.is_empty() || item_type.is_empty() || !ids.insert(id)
    }) {
        return Err(DomainError::invariant(
            ErrorCode::CodexTurnIdConflict,
            "Codex Turn contains a missing or duplicate item identity",
        ));
    }
    Ok(())
}

fn resolve_lineage(
    snapshot: &CodexThreadSnapshot,
    provider_id: &str,
    project_id: &str,
    existing: Option<&SessionRecord>,
    sessions: &[SessionRecord],
    messages: &[MessageRecord],
    turns: &[ProjectedTurn],
) -> (String, ProviderRelationshipState) {
    if let Some(SessionSource::CodexThread(source)) = existing.map(|session| &session.source) {
        return (source.lineage_id.clone(), source.relationship_state);
    }
    let independent =
        || stable_uuid(&["lineage", project_id, provider_id, &snapshot.id]).to_string();
    let Some(parent_thread_id) = snapshot.forked_from_id.as_deref() else {
        return (independent(), ProviderRelationshipState::Independent);
    };
    let Some(parent) = sessions.iter().find(|session| {
        session.project_id == project_id
            && matches!(
                &session.source,
                SessionSource::CodexThread(source)
                    if source.provider_id == provider_id
                        && source.thread_id == parent_thread_id
                        && source.history_completeness == ProviderHistoryCompleteness::Full
            )
    }) else {
        return (independent(), ProviderRelationshipState::Unresolved);
    };
    let child_turns = turns
        .iter()
        .map(|turn| (turn.snapshot.id.as_str(), turn.content_hash.as_str()))
        .collect::<Vec<_>>();
    let parent_turns = session_turns(parent, messages);
    let verified = parent_turns.len() <= child_turns.len()
        && parent_turns.iter().zip(&child_turns).all(
            |((left_id, left_hash), (right_id, right_hash))| {
                left_id == right_id && left_hash == right_hash
            },
        );
    let SessionSource::CodexThread(source) = &parent.source else {
        unreachable!("selected Codex parent")
    };
    if verified {
        (
            source.lineage_id.clone(),
            ProviderRelationshipState::Verified,
        )
    } else {
        (independent(), ProviderRelationshipState::Unresolved)
    }
}

fn session_turns(session: &SessionRecord, messages: &[MessageRecord]) -> Vec<(String, String)> {
    let by_id = messages
        .iter()
        .map(|message| (message.id.as_str(), message))
        .collect::<HashMap<_, _>>();
    let mut cursor = Some(session.current_message_id());
    let mut turns = Vec::new();
    while let Some(id) = cursor.take() {
        let Some(message) = by_id.get(id.as_str()) else {
            break;
        };
        if let Some(provenance) = message
            .data
            .as_ref()
            .and_then(|data| data.pointer("/native_message/metadata/codex"))
            && let (Some(turn_id), Some(content_hash)) = (
                provenance.get("turn_id").and_then(Value::as_str),
                provenance.get("turn_content_hash").and_then(Value::as_str),
            )
            && turns.last().is_none_or(|(last_id, _)| last_id != turn_id)
        {
            turns.push((turn_id.to_owned(), content_hash.to_owned()));
        }
        cursor.clone_from(&message.parent_message_id);
    }
    turns.reverse();
    turns
}

#[allow(
    clippy::too_many_lines,
    reason = "one auditable loop preserves Turn item order, provenance, and parent identities"
)]
fn project_messages(
    snapshot: &CodexThreadSnapshot,
    identity: &CodexImportIdentity<'_>,
    session_id: &SessionId,
    lineage_id: &str,
    turns: &[ProjectedTurn],
) -> Result<Vec<MessageRecord>, DomainError> {
    let project_id = ProjectId::new(identity.project_id);
    let root_id = MessageId::new(stable_uuid(&[
        "root",
        identity.project_id,
        identity.provider_id,
        lineage_id,
    ]));
    let root = Message {
        id: root_id,
        project_id: project_id.clone(),
        parent_message_id: None,
        role: MessageRole::System,
        kind: MessageKind::Standard,
        origin: MessageOrigin::System,
        sub_messages: vec![SubMessage::StructuredData {
            media_type: "application/vnd.ait.codex.synthetic-root+json".into(),
            value: json!({
                "provider_id": identity.provider_id,
                "lineage_id": lineage_id,
                "schema_version": PROJECTION_VERSION,
            })
            .to_string(),
        }],
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(snapshot.created_at.saturating_mul(1_000)),
    };
    root.validate()?;
    let mut messages = vec![MessageRecord::from(root)];
    let mut parent = root_id;
    for turn in turns {
        let segments = turn_segments(&turn.items);
        for (segment_index, segment) in segments.into_iter().enumerate() {
            let source_item_ids = segment
                .items
                .iter()
                .filter_map(|(_, item)| item.get("id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let sub_messages = if segment.items.is_empty() {
                vec![SubMessage::StructuredData {
                    media_type: "application/vnd.ait.codex.empty-turn+json".into(),
                    value: json!({
                        "status": turn.snapshot.status,
                        "error": turn.snapshot.error,
                    })
                    .to_string(),
                }]
            } else if segment.role == MessageRole::User {
                user_sub_messages(&segment.items)
            } else {
                provider_sub_messages(&segment.items)?
            };
            let message_id = MessageId::new(stable_uuid(&[
                "message",
                identity.project_id,
                identity.provider_id,
                lineage_id,
                &turn.snapshot.id,
                &turn.content_hash,
                &segment_index.to_string(),
                &parent.to_string(),
            ]));
            let mut metadata = DomainMetadata::default();
            metadata.0.insert(
                "codex".into(),
                json!({
                    "provider_id": identity.provider_id,
                    "observed_in_thread_id": snapshot.id,
                    "turn_id": turn.snapshot.id,
                    "turn_status": turn.snapshot.status,
                    "segment_index": segment_index,
                    "source_item_ids": source_item_ids,
                    "turn_content_hash": turn.content_hash,
                    "projection_version": PROJECTION_VERSION,
                }),
            );
            let observed_seconds = turn
                .snapshot
                .completed_at
                .or(turn.snapshot.started_at)
                .unwrap_or(snapshot.updated_at);
            let message = Message {
                id: message_id,
                project_id: project_id.clone(),
                parent_message_id: Some(parent),
                role: segment.role,
                kind: MessageKind::Standard,
                origin: MessageOrigin::Provider,
                sub_messages,
                created_by_session_id: Some(session_id.clone()),
                run_id: None,
                run_seq: None,
                tool_result: None,
                git_commit: None,
                metadata,
                created_at: TimestampMs(
                    observed_seconds
                        .saturating_mul(1_000)
                        .saturating_add(i64::try_from(segment_index).unwrap_or(i64::MAX)),
                ),
            };
            message.validate()?;
            parent = message_id;
            messages.push(MessageRecord::from(message));
        }
    }
    Ok(messages)
}

fn turn_segments(items: &[Value]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut assistant = Vec::new();
    for (ordinal, item) in items.iter().enumerate() {
        if item.get("type").and_then(Value::as_str) == Some("userMessage") {
            if !assistant.is_empty() {
                segments.push(Segment {
                    role: MessageRole::Assistant,
                    items: std::mem::take(&mut assistant),
                });
            }
            segments.push(Segment {
                role: MessageRole::User,
                items: vec![(ordinal, item.clone())],
            });
        } else {
            assistant.push((ordinal, item.clone()));
        }
    }
    if !assistant.is_empty() {
        segments.push(Segment {
            role: MessageRole::Assistant,
            items: assistant,
        });
    }
    if segments.is_empty() {
        segments.push(Segment {
            role: MessageRole::Assistant,
            items: Vec::new(),
        });
    }
    segments
}

fn user_sub_messages(items: &[(usize, Value)]) -> Vec<SubMessage> {
    items
        .iter()
        .flat_map(|(_, item)| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(user_input_parts)
        })
        .collect()
}

fn user_input_parts(input: &Value) -> Vec<SubMessage> {
    if input.get("type").and_then(Value::as_str) == Some("text")
        && let Some(text) = input.get("text").and_then(Value::as_str)
    {
        let mut parts = vec![SubMessage::Text {
            text: text.to_owned(),
        }];
        if let Some(elements) = input.get("text_elements") {
            parts.push(SubMessage::StructuredData {
                media_type: "application/vnd.openai.codex.text-elements+json".into(),
                value: elements.to_string(),
            });
        }
        return parts;
    }
    vec![SubMessage::StructuredData {
        media_type: "application/vnd.openai.codex.user-input+json".into(),
        value: input.to_string(),
    }]
}

fn provider_sub_messages(items: &[(usize, Value)]) -> Result<Vec<SubMessage>, DomainError> {
    items
        .iter()
        .map(|(ordinal, item)| {
            let external_item_id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| history_error("Codex provider item has no identity"))?;
            let item_type = item
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| history_error("Codex provider item has no type"))?;
            Ok(SubMessage::ProviderItem(ProviderItem {
                provider_kind: "codex".into(),
                external_item_id: external_item_id.to_owned(),
                item_type: item_type.to_owned(),
                ordinal: u32::try_from(*ordinal)
                    .map_err(|_| history_error("Codex Turn has too many items"))?,
                payload: bound_provider_payload(item)?,
                payload_schema_version: PROJECTION_VERSION,
            }))
        })
        .collect()
}

fn turn_content_hash(turn: &CodexTurnSnapshot, items: &[Value]) -> Result<String, DomainError> {
    let bytes = serde_json::to_vec(&json!({
        "id": turn.id,
        "status": turn.status,
        "error": sanitize_provider_value(turn.error.as_ref().unwrap_or(&Value::Null), 0),
        "items": items,
        "projection_version": PROJECTION_VERSION,
    }))
    .map_err(|_| history_error("Codex Turn cannot be normalized"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn sanitize_provider_value(value: &Value, depth: usize) -> Value {
    if depth >= MAX_PROVIDER_DEPTH {
        return Value::String("[truncated-depth]".into());
    }
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .take(MAX_PROVIDER_ARRAY_ITEMS)
                .map(|(key, value)| {
                    let lowered = key.to_ascii_lowercase();
                    let value = if ["authorization", "credential", "password", "secret", "token"]
                        .iter()
                        .any(|sensitive| lowered.contains(sensitive))
                    {
                        Value::String("[redacted]".into())
                    } else {
                        sanitize_provider_value(value, depth + 1)
                    };
                    (key.clone(), value)
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .take(MAX_PROVIDER_ARRAY_ITEMS)
                .map(|value| sanitize_provider_value(value, depth + 1))
                .collect(),
        ),
        Value::String(value) => Value::String(bounded_provider_string(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

fn bounded_provider_string(value: &str) -> String {
    value.chars().take(MAX_PROVIDER_STRING_CHARS).collect()
}

fn bound_provider_payload(value: &Value) -> Result<Value, DomainError> {
    let value = sanitize_provider_value(value, 0);
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| history_error("Codex provider item cannot be encoded"))?;
    if bytes.len() <= MAX_PROVIDER_PAYLOAD_BYTES {
        return Ok(value);
    }
    Ok(json!({
        "truncated": true,
        "byte_length": bytes.len(),
        "sha256": format!("{:x}", Sha256::digest(bytes)),
    }))
}

fn reuse_existing_projection(
    existing: &[MessageRecord],
    projected: &mut [MessageRecord],
) -> Result<(), DomainError> {
    for candidate in projected {
        if let Some(current) = existing.iter().find(|message| message.id == candidate.id) {
            if !same_projection_content(current, candidate) {
                return Err(DomainError::invariant(
                    ErrorCode::CodexHistoryReconcileConflict,
                    "Codex projection conflicts with an immutable Message identity",
                ));
            }
            *candidate = current.clone();
        }
    }
    Ok(())
}

fn same_projection_content(left: &MessageRecord, right: &MessageRecord) -> bool {
    if left.id != right.id
        || left.project_id != right.project_id
        || left.parent_message_id != right.parent_message_id
        || left.role != right.role
        || left.kind != right.kind
        || left.text != right.text
        || left.git_commit != right.git_commit
        || left
            .data
            .as_ref()
            .and_then(|value| value.pointer("/native_message/sub_messages"))
            != right
                .data
                .as_ref()
                .and_then(|value| value.pointer("/native_message/sub_messages"))
    {
        return false;
    }
    let provenance = |message: &MessageRecord, field: &str| {
        message
            .data
            .as_ref()
            .and_then(|value| value.pointer("/native_message/metadata/codex"))
            .and_then(|value| value.get(field))
            .cloned()
    };
    ["turn_id", "turn_content_hash", "segment_index"]
        .iter()
        .all(|field| provenance(left, field) == provenance(right, field))
}

fn stable_uuid(parts: &[&str]) -> Uuid {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn history_error(message: &'static str) -> DomainError {
    DomainError::invariant(ErrorCode::CodexHistoryReadFailed, message)
}

#[cfg(test)]
mod tests;
