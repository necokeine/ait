use std::{path::PathBuf, sync::Arc};

use ait_domain::{
    AgentId, DomainError, DomainMetadata, GitCommit, InstructionSourceSnapshot,
    InstructionSourceSummary, MessageId, Project, ProjectId, ProjectStatus, SessionId, SessionRoot,
    TimestampMs,
};
use ait_ports::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// One deterministic instruction layer. Larger priorities render later and win
/// when an invocation-time prompt assembler handles conflicts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstructionLayer {
    /// Always-present text supplied by the application.
    Inline {
        /// Stable source name.
        name: String,
        /// Priority; must be unique across configured layers.
        priority: u32,
        /// Instruction text.
        content: String,
    },
    /// Optional file relative to the canonical project root.
    ProjectFile {
        /// Stable source name.
        name: String,
        /// Priority; must be unique across configured layers.
        priority: u32,
        /// Relative path; absolute paths and `..` are rejected by the environment.
        path: PathBuf,
    },
    /// Optional file outside the project, allowed only with an explicit root.
    ExternalFile(ExternalInstruction),
}

impl InstructionLayer {
    fn priority(&self) -> u32 {
        match self {
            Self::Inline { priority, .. } | Self::ProjectFile { priority, .. } => *priority,
            Self::ExternalFile(source) => source.priority,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Inline { name, .. } | Self::ProjectFile { name, .. } => name,
            Self::ExternalFile(source) => &source.name,
        }
    }
}

/// Explicit authorization for an instruction file outside a Project root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalInstruction {
    /// Stable source name.
    pub name: String,
    /// Priority; must be unique across configured layers.
    pub priority: u32,
    /// Canonicalized boundary granted by the caller.
    pub authorized_root: PathBuf,
    /// Absolute path to a file beneath `authorized_root`.
    pub path: PathBuf,
}

/// Inputs needed to register a Project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRegistration {
    /// Externally assigned project identity.
    pub id: ProjectId,
    /// Human-readable name.
    pub name: String,
    /// Candidate workdir.
    pub workdir: PathBuf,
    /// Optional remote repository URL retained as declared Project provenance.
    pub repo_url: Option<String>,
}

/// Stable Project use-case failures.
#[derive(Debug, Error)]
pub enum ProjectError {
    /// Instruction priorities must be globally unique.
    #[error("duplicate instruction priority {0}")]
    DuplicateInstructionPriority(u32),
    /// An instruction source was not valid UTF-8.
    #[error("instruction source is not UTF-8: {0}")]
    InvalidInstructionEncoding(String),
    /// Human input cannot be recorded while tracked or untracked changes exist.
    #[error("project Git worktree and index must be clean before adding a user message")]
    GitDirty,
    /// The registered repository no longer has a readable HEAD.
    #[error("project repository has no readable HEAD commit")]
    GitHeadUnavailable,
    /// Declared repository URL was present but empty.
    #[error("repository URL cannot be empty")]
    InvalidRepoUrl,
    /// A constructed aggregate violated a pure domain invariant.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// Local filesystem or Git failure.
    #[error(transparent)]
    Environment(#[from] EnvironmentError),
    /// Persistence failure.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Coordinates Project registration, instruction discovery, and new Session roots.
pub struct ProjectService {
    environment: Arc<dyn ProjectEnvironment>,
    store: Arc<dyn ProjectStore>,
    instruction_layers: Vec<InstructionLayer>,
}

impl ProjectService {
    /// Creates the service and validates the deterministic source ordering.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::DuplicateInstructionPriority`] when two sources
    /// have the same priority.
    pub fn new(
        environment: Arc<dyn ProjectEnvironment>,
        store: Arc<dyn ProjectStore>,
        mut instruction_layers: Vec<InstructionLayer>,
    ) -> Result<Self, ProjectError> {
        instruction_layers.sort_by_key(InstructionLayer::priority);
        for pair in instruction_layers.windows(2) {
            if pair[0].priority() == pair[1].priority() {
                return Err(ProjectError::DuplicateInstructionPriority(
                    pair[0].priority(),
                ));
            }
        }
        Ok(Self {
            environment,
            store,
            instruction_layers,
        })
    }

    /// Registers a canonical Git root and persists its first instruction revision.
    ///
    /// # Errors
    ///
    /// Returns a [`ProjectError`] when the directory or Git root cannot be
    /// prepared, instructions cannot be read, or the Project cannot be stored.
    pub fn register_project(
        &self,
        mut registration: ProjectRegistration,
    ) -> Result<Project, ProjectError> {
        if let Some(url) = &mut registration.repo_url {
            *url = url.trim().to_owned();
            if url.is_empty() {
                return Err(ProjectError::InvalidRepoUrl);
            }
        }
        let root = self
            .environment
            .canonicalize_directory(&registration.workdir)?;
        let existing_top_level = self.environment.git_top_level(&root)?;
        let git_initialized_by_manager = existing_top_level.as_ref() != Some(&root);

        if git_initialized_by_manager {
            self.environment.git_init(&root)?;
        }

        let verified_top_level = self.environment.git_top_level(&root)?;
        if verified_top_level.as_ref() != Some(&root) {
            return Err(ProjectError::Environment(EnvironmentError::Git(format!(
                "top-level verification returned {:?}, expected {}",
                verified_top_level,
                root.display()
            ))));
        }

        let base_commit = if let Some(commit) = self.environment.git_head(&root)? {
            commit
        } else {
            self.environment.git_commit_initial(&root)?;
            self.environment.git_head(&root)?.ok_or_else(|| {
                ProjectError::Environment(EnvironmentError::Git(
                    "initial commit succeeded but HEAD is still unavailable".into(),
                ))
            })?
        };

        let instructions = self.discover_instructions(&root)?;
        let project = Project {
            id: registration.id,
            name: registration.name,
            description: String::new(),
            workdir: root,
            git_initialized_by_manager,
            repo_url: registration.repo_url,
            base_commit,
            default_agent_id: None,
            instruction_revision: 1,
            instruction_digest: instructions.content_digest.clone(),
            metadata: DomainMetadata::default(),
            status: ProjectStatus::Active,
            created_at: TimestampMs(0),
            updated_at: TimestampMs(0),
        };
        project.validate()?;
        self.store
            .register_project(project, instructions)
            .map_err(Into::into)
    }

    /// Creates a new Message tree and a Session pointing at its immutable root.
    ///
    /// The root stores a structured Project-instruction component, not a final
    /// provider prompt. Prompt assembly remains an invocation-time concern. The
    /// store reuses the current revision when the component digest matches or
    /// appends exactly one revision before atomically writing the root and Session.
    ///
    /// # Errors
    ///
    /// Returns a [`ProjectError`] when the Project is unknown, instruction
    /// discovery fails, or the atomic persistence operation fails.
    pub fn create_session(
        &self,
        project_id: ProjectId,
        agent_id: AgentId,
        session_id: SessionId,
        root_message_id: MessageId,
    ) -> Result<SessionRoot, ProjectError> {
        let project = self.store.get_project(&project_id)?;
        let instructions = self.discover_instructions(&project.workdir)?;
        self.store
            .create_session_root(CreateSessionRoot {
                project_id,
                agent_id,
                session_id,
                root_message_id,
                instructions,
            })
            .map_err(Into::into)
    }

    /// Captures the stable HEAD used as provenance for a human user Message.
    ///
    /// The worktree and index, including untracked files, must be clean. Callers
    /// should put the returned value directly on the immutable Message they
    /// append; no Git process or filesystem concern enters the domain crate.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::GitDirty`] for local changes or
    /// [`ProjectError::GitHeadUnavailable`] for an unborn/unreadable HEAD.
    pub fn capture_clean_head(&self, project_id: &ProjectId) -> Result<GitCommit, ProjectError> {
        let project = self.store.get_project(project_id)?;
        let before = self
            .environment
            .git_head(&project.workdir)?
            .ok_or(ProjectError::GitHeadUnavailable)?;
        if !self.environment.git_is_clean(&project.workdir)? {
            return Err(ProjectError::GitDirty);
        }
        let after = self
            .environment
            .git_head(&project.workdir)?
            .ok_or(ProjectError::GitHeadUnavailable)?;
        if before != after {
            return Err(ProjectError::GitHeadUnavailable);
        }
        Ok(after)
    }

    fn discover_instructions(
        &self,
        project_root: &std::path::Path,
    ) -> Result<DiscoveredInstructions, ProjectError> {
        let mut found = Vec::new();

        for layer in &self.instruction_layers {
            let (locator, bytes) = match layer {
                InstructionLayer::Inline { content, .. } => {
                    ("inline".to_owned(), Some(content.as_bytes().to_vec()))
                }
                InstructionLayer::ProjectFile { path, .. } => (
                    path.to_string_lossy().into_owned(),
                    self.environment.read_project_file(project_root, path)?,
                ),
                InstructionLayer::ExternalFile(source) => (
                    source.path.to_string_lossy().into_owned(),
                    self.environment
                        .read_authorized_file(&source.authorized_root, &source.path)?,
                ),
            };

            let Some(bytes) = bytes else { continue };
            let content = String::from_utf8(bytes.clone())
                .map_err(|_| ProjectError::InvalidInstructionEncoding(layer.name().to_owned()))?;
            found.push(InstructionSourceSnapshot {
                summary: InstructionSourceSummary {
                    name: layer.name().to_owned(),
                    locator,
                    priority: layer.priority(),
                    content_digest: sha256_hex(&bytes),
                    byte_len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                },
                content,
            });
        }

        Ok(DiscoveredInstructions {
            content_digest: instruction_component_digest(&found),
            sources: found,
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn instruction_component_digest(sources: &[InstructionSourceSnapshot]) -> String {
    let mut digest = Sha256::new();
    for source in sources {
        update_length_prefixed(&mut digest, source.summary.name.as_bytes());
        update_length_prefixed(&mut digest, source.summary.locator.as_bytes());
        digest.update(source.summary.priority.to_be_bytes());
        update_length_prefixed(&mut digest, source.summary.content_digest.as_bytes());
        digest.update(source.summary.byte_len.to_be_bytes());
        update_length_prefixed(&mut digest, source.content.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn update_length_prefixed(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}
