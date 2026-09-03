use std::{path::PathBuf, sync::Arc};

use ait_domain::{InstructionSourceSummary, MessageId, Project, ProjectId, SessionId, SessionRoot};
use ait_ports::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

const SOURCE_SEPARATOR: &str = "\n\n";

/// One deterministic instruction layer. Larger priorities render later and win
/// when instructions conflict.
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

    /// Registers a canonical Git root and persists its first prompt revision.
    ///
    /// # Errors
    ///
    /// Returns a [`ProjectError`] when the directory or Git root cannot be
    /// prepared, instructions cannot be read, or the Project cannot be stored.
    pub fn register_project(
        &self,
        registration: ProjectRegistration,
    ) -> Result<Project, ProjectError> {
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

        let instructions = self.discover_instructions(&root)?;
        let project = Project {
            id: registration.id,
            name: registration.name,
            workdir: root,
            git_initialized_by_manager,
            instruction_revision: 1,
            instruction_digest: instructions.content_digest.clone(),
        };
        self.store
            .register_project(project, instructions)
            .map_err(Into::into)
    }

    /// Creates a new Message tree and a Session pointing at its immutable root.
    ///
    /// Discovery happens before the store transaction. The store reuses the
    /// current revision when the digest matches or appends exactly one revision
    /// before atomically writing the root message and Session.
    ///
    /// # Errors
    ///
    /// Returns a [`ProjectError`] when the Project is unknown, instruction
    /// discovery fails, or the atomic persistence operation fails.
    pub fn create_session(
        &self,
        project_id: ProjectId,
        session_id: SessionId,
        root_message_id: MessageId,
    ) -> Result<SessionRoot, ProjectError> {
        let project = self.store.get_project(&project_id)?;
        let instructions = self.discover_instructions(&project.workdir)?;
        self.store
            .create_session_root(CreateSessionRoot {
                project_id,
                session_id,
                root_message_id,
                instructions,
            })
            .map_err(Into::into)
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
            found.push((
                InstructionSourceSummary {
                    name: layer.name().to_owned(),
                    locator,
                    priority: layer.priority(),
                    content_digest: sha256_hex(&bytes),
                    byte_len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                },
                content,
            ));
        }

        let mut rendered_prompt = String::new();
        for (index, (summary, content)) in found.iter().enumerate() {
            if index > 0 {
                rendered_prompt.push_str(SOURCE_SEPARATOR);
            }
            rendered_prompt.push_str("<instruction-source name=\"");
            rendered_prompt.push_str(&summary.name);
            rendered_prompt.push_str("\" priority=\"");
            rendered_prompt.push_str(&summary.priority.to_string());
            rendered_prompt.push_str("\">\n");
            rendered_prompt.push_str(content);
            if !content.ends_with('\n') {
                rendered_prompt.push('\n');
            }
            rendered_prompt.push_str("</instruction-source>");
        }

        Ok(DiscoveredInstructions {
            content_digest: sha256_hex(rendered_prompt.as_bytes()),
            sources: found.into_iter().map(|(summary, _)| summary).collect(),
            rendered_prompt,
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
