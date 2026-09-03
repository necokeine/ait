//! Contract tests for immutable Message trees and optimistic Session refs.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Barrier, Mutex},
    thread,
};

use ait_application::{MessageService, MessageServiceError};
use ait_domain::{
    DomainMetadata, Message, MessageId, MessageKind, MessageOrigin, MessageRole, ProjectId,
    ProjectedMessage, Session, SessionId, StoredMessage, SubMessage, TimestampMs,
};
use ait_ports::{MessageStore, MessageStoreError, SessionAdvance};

#[derive(Default)]
struct MemoryState {
    messages: HashMap<MessageId, Message>,
    sessions: HashMap<SessionId, Session>,
    redacted: HashSet<MessageId>,
}

#[derive(Default)]
struct MemoryMessageStore(Mutex<MemoryState>);

impl MemoryMessageStore {
    fn redact(&self, id: &MessageId) {
        self.0.lock().unwrap().redacted.insert(id.clone());
    }

    fn insert_unchecked(&self, message: Message) {
        self.0
            .lock()
            .unwrap()
            .messages
            .insert(message.id.clone(), message);
    }

    fn children_of(&self, parent: &MessageId) -> Vec<MessageId> {
        self.0
            .lock()
            .unwrap()
            .messages
            .values()
            .filter(|message| message.parent_message_id.as_ref() == Some(parent))
            .map(|message| message.id.clone())
            .collect()
    }
}

impl MessageStore for MemoryMessageStore {
    fn insert_root(&self, root: Message) -> Result<Message, MessageStoreError> {
        let mut state = self.0.lock().unwrap();
        insert_unique(&mut state, root.clone())?;
        Ok(root)
    }

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
            .ok_or_else(|| MessageStoreError::MessageNotFound(id.clone()))?;
        Ok(StoredMessage {
            redacted: state.redacted.contains(id),
            message,
        })
    }

    fn create_session(&self, session: Session) -> Result<Session, MessageStoreError> {
        let mut state = self.0.lock().unwrap();
        if state.sessions.contains_key(&session.id) {
            return Err(MessageStoreError::IdentityConflict(
                session.id.as_str().to_owned(),
            ));
        }
        let target = state
            .messages
            .get(&session.current_message_id)
            .ok_or_else(|| {
                MessageStoreError::MessageNotFound(session.current_message_id.clone())
            })?;
        if target.project_id != session.project_id {
            return Err(MessageStoreError::MessageProjectMismatch {
                expected: session.project_id,
                actual: target.project_id.clone(),
            });
        }
        state.sessions.insert(session.id.clone(), session.clone());
        Ok(session)
    }

    fn get_session(&self, id: &SessionId) -> Result<Session, MessageStoreError> {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| MessageStoreError::SessionNotFound(id.clone()))
    }

    fn append_and_advance(
        &self,
        session_id: &SessionId,
        expected_head: &MessageId,
        expected_version: u64,
        message: Message,
    ) -> Result<SessionAdvance, MessageStoreError> {
        let mut state = self.0.lock().unwrap();
        verify_parent(&state, &message)?;
        let observed = state
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| MessageStoreError::SessionNotFound(session_id.clone()))?;
        if observed.project_id != message.project_id {
            return Err(MessageStoreError::MessageProjectMismatch {
                expected: observed.project_id,
                actual: message.project_id,
            });
        }

        let preserved_message_id = message.id.clone();
        insert_unique(&mut state, message)?;
        if observed.current_message_id != *expected_head || observed.version != expected_version {
            return Ok(SessionAdvance::Conflict {
                observed,
                preserved_message_id,
            });
        }

        let session = state.sessions.get_mut(session_id).unwrap();
        session.current_message_id = preserved_message_id;
        session.version += 1;
        Ok(SessionAdvance::Advanced(session.clone()))
    }
}

fn insert_unique(state: &mut MemoryState, message: Message) -> Result<(), MessageStoreError> {
    if state.messages.contains_key(&message.id) {
        return Err(MessageStoreError::IdentityConflict(
            message.id.as_str().to_owned(),
        ));
    }
    state.messages.insert(message.id.clone(), message);
    Ok(())
}

fn verify_parent(state: &MemoryState, message: &Message) -> Result<(), MessageStoreError> {
    let parent_id = message
        .parent_message_id
        .as_ref()
        .ok_or_else(|| MessageStoreError::Other("parent required".into()))?;
    let parent = state
        .messages
        .get(parent_id)
        .ok_or_else(|| MessageStoreError::MessageNotFound(parent_id.clone()))?;
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

fn root(id: &str, project_id: &str) -> Message {
    Message {
        id: MessageId::new(id),
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
        id: MessageId::new(id),
        project_id: project(project_id),
        parent_message_id: Some(MessageId::new(parent)),
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

fn fixture() -> (Arc<MemoryMessageStore>, Arc<MessageService>) {
    let store = Arc::new(MemoryMessageStore::default());
    let service = Arc::new(MessageService::new(store.clone()));
    (store, service)
}

fn visible_id(message: &ProjectedMessage) -> &str {
    match message {
        ProjectedMessage::Visible(message) => message.id.as_str(),
        ProjectedMessage::Redacted { id, .. } => id.as_str(),
    }
}

#[test]
fn creates_a_root_and_projects_an_ordered_path_from_any_head() {
    let (_store, service) = fixture();
    service.create_root(root("m0", "p1")).unwrap();
    service
        .append(child("m1", "p1", "m0", MessageRole::User, None))
        .unwrap();
    service
        .append(child("m2", "p1", "m1", MessageRole::Assistant, None))
        .unwrap();

    let path = service.message_path(&MessageId::new("m2")).unwrap();

    assert_eq!(
        path.iter().map(visible_id).collect::<Vec<_>>(),
        vec!["m0", "m1", "m2"]
    );
}

#[test]
fn a_session_can_continue_a_message_created_by_another_session() {
    let (_store, service) = fixture();
    service.create_root(root("m0", "p1")).unwrap();
    service
        .open_session(SessionId::new("s1"), project("p1"), MessageId::new("m0"))
        .unwrap();
    service
        .append_to_session(
            &SessionId::new("s1"),
            &MessageId::new("m0"),
            1,
            child("m1", "p1", "m0", MessageRole::User, Some("s1")),
        )
        .unwrap();

    service
        .open_session(SessionId::new("s2"), project("p1"), MessageId::new("m1"))
        .unwrap();
    service
        .append_to_session(
            &SessionId::new("s2"),
            &MessageId::new("m1"),
            1,
            child("m2", "p1", "m1", MessageRole::User, Some("s2")),
        )
        .unwrap();

    let view = service.session_view(&SessionId::new("s2")).unwrap();
    assert_eq!(
        view.messages.iter().map(visible_id).collect::<Vec<_>>(),
        vec!["m0", "m1", "m2"]
    );
}

#[test]
fn opening_a_head_in_another_project_is_rejected() {
    let (_store, service) = fixture();
    service.create_root(root("m0", "p1")).unwrap();

    let result = service.open_session(SessionId::new("s1"), project("p2"), MessageId::new("m0"));

    assert!(matches!(
        result,
        Err(MessageServiceError::ProjectMismatch { .. })
    ));
}

#[test]
fn corrupted_parent_cycles_are_detected_instead_of_looping() {
    let (store, service) = fixture();
    store.insert_unchecked(child("m1", "p1", "m2", MessageRole::Assistant, None));
    store.insert_unchecked(child("m2", "p1", "m1", MessageRole::User, None));

    let result = service.message_path(&MessageId::new("m2"));

    assert!(matches!(
        result,
        Err(MessageServiceError::CycleDetected(id)) if id == MessageId::new("m2")
    ));
}

#[test]
fn concurrent_cas_keeps_the_losing_message_as_a_sibling_branch() {
    let (store, service) = fixture();
    service.create_root(root("m0", "p1")).unwrap();
    service
        .open_session(SessionId::new("s1"), project("p1"), MessageId::new("m0"))
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();

    for id in ["m1", "m2"] {
        let service = service.clone();
        let barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            let message = child(id, "p1", "m0", MessageRole::User, Some("s1"));
            barrier.wait();
            service.append_to_session(&SessionId::new("s1"), &MessageId::new("m0"), 1, message)
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
    assert!(store.get_message(conflict).is_ok());
    let mut children = store.children_of(&MessageId::new("m0"));
    children.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    assert_eq!(children, vec![MessageId::new("m1"), MessageId::new("m2")]);
}

#[test]
fn editing_and_regeneration_append_siblings_without_mutating_history() {
    let (store, service) = fixture();
    service.create_root(root("m0", "p1")).unwrap();
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
        store.get_message(&original_user.id).unwrap().message,
        original_user
    );
    assert_eq!(
        store.get_message(&original_assistant.id).unwrap().message,
        original_assistant
    );
    assert_eq!(store.get_message(&edited.id).unwrap().message, edited);
    assert_eq!(
        store.get_message(&regenerated.id).unwrap().message,
        regenerated
    );
}

#[test]
fn redacted_nodes_keep_their_place_without_exposing_content() {
    let (store, service) = fixture();
    service.create_root(root("m0", "p1")).unwrap();
    service
        .append(child("m1", "p1", "m0", MessageRole::User, None))
        .unwrap();
    service
        .append(child("m2", "p1", "m1", MessageRole::Assistant, None))
        .unwrap();
    store.redact(&MessageId::new("m1"));
    service
        .open_session(SessionId::new("s1"), project("p1"), MessageId::new("m2"))
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
        } if id == &MessageId::new("m1") && parent == &MessageId::new("m0")
    ));
    assert_eq!(visible_id(&view.messages[2]), "m2");
}
