use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use ait_application::{InstructionLayer, ProjectRegistration, ProjectService};
use ait_domain::{
    AgentId, InstructionSnapshot, MessageId, Project, ProjectId, Session, SessionId, SessionRoot,
    SystemMessage, SystemMessageComponent, TimestampMs,
};
use ait_ports::{
    CreateSessionRoot, DiscoveredInstructions, ProjectEnvironment, ProjectStore, StoreError,
};
use tempfile::TempDir;

use super::{LocalProjectEnvironment, ProjectPathGuard};

fn message_id(value: u128) -> MessageId {
    MessageId::from_u128(value)
}

#[derive(Default)]
struct MemoryState {
    projects: HashMap<ProjectId, Project>,
    roots: HashMap<PathBuf, ProjectId>,
    revisions: HashMap<ProjectId, Vec<InstructionSnapshot>>,
    messages: HashMap<MessageId, SystemMessage>,
    sessions: HashMap<SessionId, Session>,
}

#[derive(Default)]
struct MemoryProjectStore(Mutex<MemoryState>);

impl MemoryProjectStore {
    fn message(&self, id: &MessageId) -> SystemMessage {
        self.0.lock().unwrap().messages[id].clone()
    }
}

impl ProjectStore for MemoryProjectStore {
    fn register_project(
        &self,
        mut project: Project,
        instructions: DiscoveredInstructions,
    ) -> Result<Project, StoreError> {
        let mut state = self.0.lock().unwrap();
        if state.roots.contains_key(&project.workdir) {
            return Err(StoreError::ProjectPathAlreadyRegistered(project.workdir));
        }
        if state.projects.contains_key(&project.id) {
            return Err(StoreError::IdentityConflict(project.id.as_str().to_owned()));
        }
        project.instruction_revision = 1;
        project.instruction_digest = instructions.content_digest.clone();
        state
            .roots
            .insert(project.workdir.clone(), project.id.clone());
        state.revisions.insert(
            project.id.clone(),
            vec![InstructionSnapshot {
                revision: 1,
                sources: instructions.sources,
                content_digest: instructions.content_digest,
            }],
        );
        state.projects.insert(project.id.clone(), project.clone());
        Ok(project)
    }

    fn get_project(&self, project_id: &ProjectId) -> Result<Project, StoreError> {
        self.0
            .lock()
            .unwrap()
            .projects
            .get(project_id)
            .cloned()
            .ok_or_else(|| StoreError::ProjectNotFound(project_id.clone()))
    }

    fn create_session_root(&self, command: CreateSessionRoot) -> Result<SessionRoot, StoreError> {
        let mut state = self.0.lock().unwrap();
        if state.messages.contains_key(&command.root_message_id) {
            return Err(StoreError::IdentityConflict(
                command.root_message_id.to_string(),
            ));
        }
        if state.sessions.contains_key(&command.session_id) {
            return Err(StoreError::IdentityConflict(
                command.session_id.as_str().to_owned(),
            ));
        }
        let project = state
            .projects
            .get(&command.project_id)
            .cloned()
            .ok_or_else(|| StoreError::ProjectNotFound(command.project_id.clone()))?;

        let snapshot = if project.instruction_digest == command.instructions.content_digest {
            state.revisions[&command.project_id].last().unwrap().clone()
        } else {
            let revision = project.instruction_revision + 1;
            let snapshot = InstructionSnapshot {
                revision,
                sources: command.instructions.sources,
                content_digest: command.instructions.content_digest,
            };
            state
                .revisions
                .get_mut(&command.project_id)
                .unwrap()
                .push(snapshot.clone());
            let project = state.projects.get_mut(&command.project_id).unwrap();
            project.instruction_revision = revision;
            project
                .instruction_digest
                .clone_from(&snapshot.content_digest);
            snapshot
        };

        let root_message = SystemMessage {
            id: command.root_message_id,
            project_id: command.project_id.clone(),
            components: vec![SystemMessageComponent::ProjectInstructions(snapshot)],
        };
        let session_name = command.session_id.as_str().to_owned();
        let session = Session::new(
            command.session_id,
            command.project_id,
            session_name,
            root_message.id,
            command.agent_id,
            TimestampMs(0),
        );
        state.messages.insert(root_message.id, root_message.clone());
        state.sessions.insert(session.id.clone(), session.clone());
        Ok(SessionRoot {
            session,
            root_message,
        })
    }
}

fn service(store: Arc<MemoryProjectStore>, layers: Vec<InstructionLayer>) -> ProjectService {
    ProjectService::new(Arc::new(LocalProjectEnvironment), store, layers).unwrap()
}

fn registration(path: &Path, id: &str) -> ProjectRegistration {
    ProjectRegistration {
        id: ProjectId::new(id),
        name: id.to_owned(),
        workdir: path.to_path_buf(),
        repo_url: None,
    }
}

#[test]
fn registration_canonicalizes_and_initializes_an_independent_git_root() {
    let temp = TempDir::new().unwrap();
    run_git(temp.path(), &["init", "--quiet"]);
    let nested = temp.path().join("nested/project");
    fs::create_dir_all(&nested).unwrap();
    let store = Arc::new(MemoryProjectStore::default());
    let service = service(store, vec![]);

    let project = service
        .register_project(registration(&nested, "p1"))
        .unwrap();

    assert_eq!(project.workdir, fs::canonicalize(&nested).unwrap());
    assert!(project.git_initialized_by_manager);
    assert_eq!(project.base_commit.as_str().len(), 40);
    let top = Command::new("git")
        .arg("-C")
        .arg(&nested)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    assert_eq!(
        fs::canonicalize(String::from_utf8(top.stdout).unwrap().trim()).unwrap(),
        project.workdir
    );
}

#[test]
fn clean_head_is_captured_and_dirty_input_is_rejected() {
    let temp = TempDir::new().unwrap();
    let store = Arc::new(MemoryProjectStore::default());
    let service = service(Arc::clone(&store), vec![]);
    let project = service
        .register_project(ProjectRegistration {
            id: ProjectId::new("p1"),
            name: "p1".into(),
            workdir: temp.path().to_path_buf(),
            repo_url: Some("git@github.com:member/fork.git".into()),
        })
        .unwrap();

    assert_eq!(
        service.capture_clean_head(&project.id).unwrap(),
        project.base_commit
    );
    assert_eq!(
        project.repo_url.as_deref(),
        Some("git@github.com:member/fork.git")
    );

    fs::write(temp.path().join("dirty.txt"), "not committed").unwrap();
    assert!(matches!(
        service.capture_clean_head(&project.id),
        Err(ait_application::ProjectError::GitDirty)
    ));
}

#[test]
fn canonical_aliases_cannot_be_registered_twice() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let alias = project.join(".");
    let store = Arc::new(MemoryProjectStore::default());
    let service = service(store, vec![]);
    service
        .register_project(registration(&project, "p1"))
        .unwrap();

    let duplicate = service.register_project(registration(&alias, "p2"));

    assert!(matches!(
        duplicate,
        Err(ait_application::ProjectError::Store(
            StoreError::ProjectPathAlreadyRegistered(_)
        ))
    ));
}

#[test]
fn missing_sources_are_skipped_and_conflicts_remain_priority_ordered() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("override.md"), "policy = high").unwrap();
    let store = Arc::new(MemoryProjectStore::default());
    let service = service(
        Arc::clone(&store),
        vec![
            InstructionLayer::ProjectFile {
                name: "missing".into(),
                priority: 20,
                path: "missing.md".into(),
            },
            InstructionLayer::ProjectFile {
                name: "override".into(),
                priority: 30,
                path: "override.md".into(),
            },
            InstructionLayer::Inline {
                name: "base".into(),
                priority: 10,
                content: "policy = low".into(),
            },
        ],
    );
    service
        .register_project(registration(temp.path(), "p1"))
        .unwrap();

    let created = service
        .create_session(
            ProjectId::new("p1"),
            AgentId::new("a1"),
            SessionId::new("s1"),
            message_id(1),
        )
        .unwrap();

    let SystemMessageComponent::ProjectInstructions(snapshot) = &created.root_message.components[0];
    assert_eq!(snapshot.sources.len(), 2);
    assert_eq!(snapshot.sources[0].summary.name, "base");
    assert_eq!(snapshot.sources[0].content, "policy = low");
    assert_eq!(snapshot.sources[1].summary.name, "override");
    assert_eq!(snapshot.sources[1].content, "policy = high");
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_for_reads_and_creates() {
    use std::os::unix::fs::symlink;

    let project = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.md"), "secret").unwrap();
    symlink(outside.path(), project.path().join("escape")).unwrap();
    let environment = LocalProjectEnvironment;
    let guard = ProjectPathGuard::new(project.path()).unwrap();

    let read = environment.read_project_file(project.path(), Path::new("escape/secret.md"));
    assert!(matches!(
        read,
        Err(ait_ports::EnvironmentError::OutOfScope(_))
    ));
    let create = guard.resolve_for_creation(Path::new("escape/new.txt"));
    assert!(matches!(
        create,
        Err(ait_ports::EnvironmentError::OutOfScope(_))
    ));
}

#[test]
fn missing_external_file_outside_the_authorized_root_is_still_rejected() {
    let authorized = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let missing = outside.path().join("missing.md");
    let environment = LocalProjectEnvironment;

    let result = environment.read_authorized_file(authorized.path(), &missing);

    assert!(matches!(
        result,
        Err(ait_ports::EnvironmentError::OutOfScope(_))
    ));
}

#[test]
fn instruction_updates_append_a_revision_without_mutating_old_session_roots() {
    let temp = TempDir::new().unwrap();
    let instructions = temp.path().join("AGENTS.md");
    fs::write(&instructions, "version one").unwrap();
    let store = Arc::new(MemoryProjectStore::default());
    let service = service(
        Arc::clone(&store),
        vec![InstructionLayer::ProjectFile {
            name: "project".into(),
            priority: 100,
            path: "AGENTS.md".into(),
        }],
    );
    service
        .register_project(registration(temp.path(), "p1"))
        .unwrap();
    let first = service
        .create_session(
            ProjectId::new("p1"),
            AgentId::new("a1"),
            SessionId::new("s1"),
            message_id(1),
        )
        .unwrap();

    fs::write(&instructions, "version two").unwrap();
    let second = service
        .create_session(
            ProjectId::new("p1"),
            AgentId::new("a1"),
            SessionId::new("s2"),
            message_id(2),
        )
        .unwrap();
    let unchanged = service
        .create_session(
            ProjectId::new("p1"),
            AgentId::new("a1"),
            SessionId::new("s3"),
            message_id(3),
        )
        .unwrap();

    let SystemMessageComponent::ProjectInstructions(first_snapshot) =
        &first.root_message.components[0];
    let SystemMessageComponent::ProjectInstructions(second_snapshot) =
        &second.root_message.components[0];
    let SystemMessageComponent::ProjectInstructions(unchanged_snapshot) =
        &unchanged.root_message.components[0];
    assert_eq!(first_snapshot.revision, 1);
    assert_eq!(first_snapshot.sources[0].content, "version one");
    assert_eq!(second_snapshot.revision, 2);
    assert_eq!(second_snapshot.sources[0].content, "version two");
    assert_eq!(unchanged_snapshot.revision, 2);
    assert_eq!(store.message(&message_id(1)), first.root_message);
}

#[test]
fn duplicate_priorities_are_rejected_at_configuration_time() {
    let store = Arc::new(MemoryProjectStore::default());
    let result = ProjectService::new(
        Arc::new(LocalProjectEnvironment),
        store,
        vec![
            InstructionLayer::Inline {
                name: "one".into(),
                priority: 1,
                content: String::new(),
            },
            InstructionLayer::Inline {
                name: "two".into(),
                priority: 1,
                content: String::new(),
            },
        ],
    );
    assert!(matches!(
        result,
        Err(ait_application::ProjectError::DuplicateInstructionPriority(
            1
        ))
    ));
}

fn run_git(directory: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}
