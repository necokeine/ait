//! Contract tests for immutable Message trees and optimistic Session refs.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Barrier, Mutex},
    thread,
};

use ait_application::{MessageService, MessageServiceError};
use ait_domain::{
    AgentId, DomainMetadata, Message, MessageId, MessageKind, MessageOrigin, MessageRole,
    ProjectId, ProjectedMessage, Session, SessionId, StoredMessage, SubMessage, TimestampMs,
};
use ait_ports::{MessageStore, MessageStoreError, SessionAdvance, SessionStore, SessionStoreError};

#[derive(Default)]
struct MemoryMessageState {
    messages: HashMap<MessageId, Message>,
    redacted: HashSet<MessageId>,
}

struct MemoryMessageStore(Mutex<MemoryMessageState>);

impl MemoryMessageStore {
    fn new(root: Message) -> Self {
        root.validate().unwrap();
        assert_eq!(root.role, MessageRole::System);
        assert!(root.parent_message_id.is_none());
        let id = root.id;
        let store = Self(Mutex::new(MemoryMessageState::default()));
        store.0.lock().unwrap().messages.insert(id, root);
        store
    }

    fn redact(&self, id: &MessageId) {
        self.0.lock().unwrap().redacted.insert(*id);
    }

    fn insert_unchecked(&self, message: Message) {
        self.0.lock().unwrap().messages.insert(message.id, message);
    }

    fn children_of(&self, parent: &MessageId) -> Vec<MessageId> {
        self.0
            .lock()
            .unwrap()
            .messages
            .values()
            .filter(|message| message.parent_message_id.as_ref() == Some(parent))
            .map(|message| message.id)
            .collect()
    }
}

impl MessageStore for MemoryMessageStore {
    fn append_message(&self, message: Message) -> Result<Message, MessageStoreError> {
        let mut state = self.0.lock().unwrap();
        verify_parent(&state, &message)?;
        insert_unique(&mut state, message.clone())?;
        Ok(message)
    }

    fn get_message(&self, id: &MessageId) -> Result<StoredMessage, MessageStoreError> {
        let state = self.0.lock().unwrap();
        let message = state
            .messages
            .get(id)
            .cloned()
            .ok_or(MessageStoreError::MessageNotFound(*id))?;
        Ok(StoredMessage {
            redacted: state.redacted.contains(id),
            message,
        })
    }
}

#[derive(Default)]
struct MemorySessionState {
    sessions: HashMap<SessionId, Session>,
    fail_next_advance: bool,
}

#[derive(Default)]
struct MemorySessionStore(Mutex<MemorySessionState>);

impl MemorySessionStore {
    fn fail_next_advance(&self) {
        self.0.lock().unwrap().fail_next_advance = true;
    }
}

impl SessionStore for MemorySessionStore {
    fn create_session(&self, session: Session) -> Result<Session, SessionStoreError> {
        let mut state = self.0.lock().unwrap();
        if state.sessions.contains_key(&session.id) {
            return Err(SessionStoreError::IdentityConflict(session.id));
        }
        state.sessions.insert(session.id.clone(), session.clone());
        Ok(session)
    }

    fn get_session(&self, id: &SessionId) -> Result<Session, SessionStoreError> {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| SessionStoreError::SessionNotFound(id.clone()))
    }

    fn advance_head(
        &self,
        session_id: &SessionId,
        expected_head: &MessageId,
        expected_version: u64,
        new_head: &MessageId,
    ) -> Result<SessionAdvance, SessionStoreError> {
        let mut state = self.0.lock().unwrap();
        if state.fail_next_advance {
            state.fail_next_advance = false;
            return Err(SessionStoreError::Other("injected update failure".into()));
        }
        let observed = state
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::SessionNotFound(session_id.clone()))?;
        if observed.current_message_id != *expected_head || observed.version != expected_version {
            return Ok(SessionAdvance::Conflict { observed });
        }

        let session = state.sessions.get_mut(session_id).unwrap();
        session.current_message_id = *new_head;
        session.version += 1;
        Ok(SessionAdvance::Advanced(session.clone()))
    }
}

fn insert_unique(
    state: &mut MemoryMessageState,
    message: Message,
) -> Result<(), MessageStoreError> {
    if state.messages.contains_key(&message.id) {
        return Err(MessageStoreError::IdentityConflict(message.id.to_string()));
    }
    state.messages.insert(message.id, message);
    Ok(())
}

fn verify_parent(state: &MemoryMessageState, message: &Message) -> Result<(), MessageStoreError> {
    let parent_id = message
        .parent_message_id
        .as_ref()
        .ok_or_else(|| MessageStoreError::Other("parent required".into()))?;
    let parent = state
        .messages
        .get(parent_id)
        .ok_or(MessageStoreError::MessageNotFound(*parent_id))?;
    if parent.project_id != message.project_id {
        return Err(MessageStoreError::MessageProjectMismatch {
            expected: message.project_id.clone(),
            actual: parent.project_id.clone(),
        });
    }
    Ok(())
}

fn project(value: &str) -> ProjectId {
    ProjectId::new(value)
}

fn agent(value: &str) -> AgentId {
    AgentId::new(value)
}

fn message_id(value: &str) -> MessageId {
    let raw = value.bytes().fold(0_u128, |current, byte| {
        current.wrapping_mul(257).wrapping_add(u128::from(byte))
    });
    MessageId::from_u128(raw)
}

fn root(id: &str, project_id: &str) -> Message {
    Message {
        id: message_id(id),
        project_id: project(project_id),
        parent_message_id: None,
        role: MessageRole::System,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Project,
        sub_messages: vec![SubMessage::Text {
            text: "system".into(),
        }],
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(0),
    }
}

fn child(
    id: &str,
    project_id: &str,
    parent: &str,
    role: MessageRole,
    session_id: Option<&str>,
) -> Message {
    Message {
        id: message_id(id),
        project_id: project(project_id),
        parent_message_id: Some(message_id(parent)),
        role,
        kind: MessageKind::Standard,
        origin: match role {
            MessageRole::User => MessageOrigin::Human,
            MessageRole::System => MessageOrigin::System,
            MessageRole::Assistant => MessageOrigin::Agent,
        },
        sub_messages: vec![SubMessage::Text { text: id.into() }],
        created_by_session_id: session_id.map(SessionId::new),
        run_id: None,
        run_seq: None,
        tool_result: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(0),
    }
}

fn fixture() -> (
    Arc<MemoryMessageStore>,
    Arc<MemorySessionStore>,
    Arc<MessageService>,
) {
    let messages = Arc::new(MemoryMessageStore::new(root("m0", "p1")));
    let sessions = Arc::new(MemorySessionStore::default());
    let service = Arc::new(MessageService::new(messages.clone(), sessions.clone()));
    (messages, sessions, service)
}

fn visible_id(message: &ProjectedMessage) -> MessageId {
    match message {
        ProjectedMessage::Visible(message) => message.id,
        ProjectedMessage::Redacted { id, .. } => *id,
    }
}

#[test]
fn initialized_store_projects_an_ordered_path_from_any_head() {
    let (_messages, _sessions, service) = fixture();
    service
        .append(child("m1", "p1", "m0", MessageRole::User, None))
        .unwrap();
    service
        .append(child("m2", "p1", "m1", MessageRole::Assistant, None))
        .unwrap();

    let path = service.message_path(&message_id("m2")).unwrap();

    assert_eq!(
        path.iter().map(visible_id).collect::<Vec<_>>(),
        vec![message_id("m0"), message_id("m1"), message_id("m2")]
    );
}

#[test]
fn a_session_can_continue_a_message_created_by_another_session() {
    let (_messages, _sessions, service) = fixture();
    service
        .open_session(
            SessionId::new("s1"),
            project("p1"),
            message_id("m0"),
            agent("a1"),
        )
        .unwrap();
    service
        .append_to_session(
            &SessionId::new("s1"),
            &message_id("m0"),
            1,
            child("m1", "p1", "m0", MessageRole::User, Some("s1")),
        )
        .unwrap();

    service
        .open_session(
            SessionId::new("s2"),
            project("p1"),
            message_id("m1"),
            agent("a1"),
        )
        .unwrap();
    service
        .append_to_session(
            &SessionId::new("s2"),
            &message_id("m1"),
            1,
            child("m2", "p1", "m1", MessageRole::User, Some("s2")),
        )
        .unwrap();

    let view = service.session_view(&SessionId::new("s2")).unwrap();
    assert_eq!(
        view.messages.iter().map(visible_id).collect::<Vec<_>>(),
        vec![message_id("m0"), message_id("m1"), message_id("m2")]
    );
}

#[test]
fn opening_a_head_in_another_project_is_rejected() {
    let (_messages, _sessions, service) = fixture();

    let result = service.open_session(
        SessionId::new("s1"),
        project("p2"),
        message_id("m0"),
        agent("a1"),
    );

    assert!(matches!(
        result,
        Err(MessageServiceError::ProjectMismatch { .. })
    ));
}

#[test]
fn corrupted_parent_cycles_are_detected_instead_of_looping() {
    let (messages, _sessions, service) = fixture();
    messages.insert_unchecked(child("m1", "p1", "m2", MessageRole::Assistant, None));
    messages.insert_unchecked(child("m2", "p1", "m1", MessageRole::User, None));

    let result = service.message_path(&message_id("m2"));

    assert!(matches!(
        result,
        Err(MessageServiceError::CycleDetected(id)) if id == message_id("m2")
    ));
}

#[test]
fn concurrent_cas_keeps_the_losing_message_as_a_sibling_branch() {
    let (messages, _sessions, service) = fixture();
    service
        .open_session(
            SessionId::new("s1"),
            project("p1"),
            message_id("m0"),
            agent("a1"),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();

    for id in ["m1", "m2"] {
        let service = service.clone();
        let barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            let message = child(id, "p1", "m0", MessageRole::User, Some("s1"));
            barrier.wait();
            service.append_to_session(&SessionId::new("s1"), &message_id("m0"), 1, message)
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let conflict = results
        .iter()
        .find_map(|result| match result {
            Err(MessageServiceError::PointerConflict {
                preserved_message_id,
                ..
            }) => Some(preserved_message_id),
            _ => None,
        })
        .expect("one writer must lose the compare-and-swap");
    assert!(messages.get_message(conflict).is_ok());
    let mut children = messages.children_of(&message_id("m0"));
    children.sort();
    assert_eq!(children, vec![message_id("m1"), message_id("m2")]);
}

#[test]
fn session_update_failure_reports_the_message_that_was_already_preserved() {
    let (messages, sessions, service) = fixture();
    service
        .open_session(
            SessionId::new("s1"),
            project("p1"),
            message_id("m0"),
            agent("a1"),
        )
        .unwrap();
    sessions.fail_next_advance();

    let result = service.append_to_session(
        &SessionId::new("s1"),
        &message_id("m0"),
        1,
        child("m1", "p1", "m0", MessageRole::User, Some("s1")),
    );

    assert!(matches!(
        result,
        Err(MessageServiceError::SessionUpdateFailed {
            preserved_message_id,
            ..
        }) if preserved_message_id == message_id("m1")
    ));
    assert!(messages.get_message(&message_id("m1")).is_ok());
    let unchanged = sessions.get_session(&SessionId::new("s1")).unwrap();
    assert_eq!(unchanged.current_message_id, message_id("m0"));
    assert_eq!(unchanged.version, 1);
}

#[test]
fn editing_and_regeneration_append_siblings_without_mutating_history() {
    let (messages, _sessions, service) = fixture();
    let original_user = child("u1", "p1", "m0", MessageRole::User, Some("s1"));
    let original_assistant = child("a1", "p1", "u1", MessageRole::Assistant, Some("s1"));
    service.append(original_user.clone()).unwrap();
    service.append(original_assistant.clone()).unwrap();

    let edited = child("u2", "p1", "m0", MessageRole::User, Some("s2"));
    let regenerated = child("a2", "p1", "u1", MessageRole::Assistant, Some("s3"));
    service
        .fork_edit(&original_user.id, edited.clone())
        .unwrap();
    service
        .fork_regeneration(&original_assistant.id, regenerated.clone())
        .unwrap();

    assert_eq!(
        messages.get_message(&original_user.id).unwrap().message,
        original_user
    );
    assert_eq!(
        messages
            .get_message(&original_assistant.id)
            .unwrap()
            .message,
        original_assistant
    );
    assert_eq!(messages.get_message(&edited.id).unwrap().message, edited);
    assert_eq!(
        messages.get_message(&regenerated.id).unwrap().message,
        regenerated
    );
}

#[test]
fn redacted_nodes_keep_their_place_without_exposing_content() {
    let (messages, _sessions, service) = fixture();
    service
        .append(child("m1", "p1", "m0", MessageRole::User, None))
        .unwrap();
    service
        .append(child("m2", "p1", "m1", MessageRole::Assistant, None))
        .unwrap();
    messages.redact(&message_id("m1"));
    service
        .open_session(
            SessionId::new("s1"),
            project("p1"),
            message_id("m2"),
            agent("a1"),
        )
        .unwrap();

    let view = service.session_view(&SessionId::new("s1")).unwrap();

    assert_eq!(view.messages.len(), 3);
    assert!(matches!(
        &view.messages[1],
        ProjectedMessage::Redacted {
            id,
            parent_message_id: Some(parent),
            role: MessageRole::User,
            ..
        } if id == &message_id("m1") && parent == &message_id("m0")
    ));
    assert_eq!(visible_id(&view.messages[2]), message_id("m2"));
}
