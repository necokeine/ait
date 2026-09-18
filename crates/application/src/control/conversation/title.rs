//! Session metadata and first-interaction title generation.
use crate::control::LocalControlService;
use crate::control::conversation::SessionRecord;
use crate::control::errors::{error, store_error};
use crate::control::events::pending;
use crate::control::persistence::{
    HasAgents, HasMessages, HasProviderCredentials, HasProviders, HasRuns, HasSessions, HasSettings,
};
use crate::control::settings::{
    DEFAULT_AGENT_SETTING_ID, SMALL_AGENT_SETTING_ID, configured_agent_id,
};
#[cfg(all(feature = "dev-mock-provider", debug_assertions))]
use ait_contracts::AgentMode;
use ait_contracts::{ApiError, CommandResult, Response};
use ait_domain::ErrorCode;
use ait_ports::{ControlStoreError, PendingEvent, SessionTitleRequest};
use uuid::Uuid;

struct TitleAgent {
    config: ait_domain::AgentConfiguration,
    provider: ait_domain::AgentProvider,
    credential_ref: Option<String>,
}

pub(in crate::control) fn set_session_title(
    state: &mut impl HasSessions,
    session_id: &str,
    title: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 60 {
        return Err(error(
            ErrorCode::InvalidSession,
            "Temporary Session title must contain 1 to 60 characters",
            false,
        ));
    }
    let session = state
        .sessions_mut()
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if !session.title_generation_started {
        session.title = Some(title);
    }
    let session = session.clone();
    Ok((
        CommandResult::Session(session.view()),
        vec![pending(
            "session.title_updated",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn is_first_completed_interaction(
    state: &(impl HasMessages + HasRuns),
    session: &SessionRecord,
) -> bool {
    let head_is_assistant = state
        .messages()
        .iter()
        .find(|message| message.id == session.current_message_id())
        .is_some_and(|message| message.role == ait_domain::MessageRole::Assistant);
    head_is_assistant
        && state
            .runs()
            .iter()
            .filter(|run| run.session_id.as_deref() == Some(session.id.as_str()))
            .count()
            == 1
}

fn validate_session_metadata(title: &str, description: &str) -> Result<(), ApiError> {
    let title = title.trim();
    let invalid_markup = [
        '"', '\'', '“', '”', '‘', '’', '#', '*', '`', '[', ']', '<', '>',
    ];
    let ending_punctuation = [
        '.', '。', '!', '！', '?', '？', ',', '，', ';', '；', ':', '：',
    ];
    if title.is_empty()
        || title.chars().count() > 36
        || title
            .chars()
            .any(|character| invalid_markup.contains(&character))
        || title
            .chars()
            .last()
            .is_some_and(|character| ending_punctuation.contains(&character))
        || description.trim().is_empty()
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "generated Session metadata is outside the requested constraints",
            false,
        ));
    }
    Ok(())
}

impl LocalControlService {
    /// Runs the one-shot background title turn after a Session's first interaction.
    pub async fn generate_session_title(
        &self,
        session_id: String,
        user_prompt: String,
    ) -> Response {
        match self
            .try_generate_session_title(&session_id, &user_prompt)
            .await
        {
            Ok(session) => Response::success(CommandResult::Session(session.view())),
            Err(error) => Response::failure(error),
        }
    }

    async fn try_generate_session_title(
        &self,
        session_id: &str,
        user_prompt: &str,
    ) -> Result<SessionRecord, ApiError> {
        let bounded_prompt = user_prompt.chars().take(2_000).collect::<String>();
        if bounded_prompt.trim().is_empty() {
            return Err(error(
                ErrorCode::InvalidSession,
                "Session title prompt is empty",
                false,
            ));
        }
        let (session, workdir, should_generate, local_only, title_agent) =
            self.begin_title_generation(session_id).await?;
        if !should_generate || local_only {
            return Ok(session);
        }
        let generator = self.session_title_generator.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "Session title generator is not configured",
                false,
            )
        })?;
        let generated = generator
            .generate(SessionTitleRequest {
                request_id: format!("session-title-{}", Uuid::new_v4()),
                user_prompt: bounded_prompt,
                config: title_agent.config,
                provider: title_agent.provider,
                credential_ref: title_agent.credential_ref,
                cwd: workdir.into(),
                cancellation: tokio_util::sync::CancellationToken::new(),
            })
            .await
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        validate_session_metadata(&generated.title, &generated.description)?;
        self.finish_title_generation(session_id, generated.title, generated.description)
            .await
    }

    async fn begin_title_generation(
        &self,
        session_id: &str,
    ) -> Result<(SessionRecord, String, bool, bool, TitleAgent), ApiError> {
        for _ in 0..4 {
            let loaded = self.read_session_title_records(session_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .sessions
                .iter()
                .position(|session| session.id == session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let session = state.sessions[index].clone();
            let _project = state
                .projects
                .iter()
                .find(|project| project.id == session.project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            let title_agent_id = configured_agent_id(state.settings(), SMALL_AGENT_SETTING_ID)
                .or_else(|| configured_agent_id(state.settings(), DEFAULT_AGENT_SETTING_ID))
                .unwrap_or_else(|| session.agent_id());
            let agent = state
                .agents()
                .iter()
                .find(|agent| agent.id == title_agent_id && agent.enabled)
                .ok_or_else(|| {
                    error(
                        ErrorCode::InvalidAgentConfiguration,
                        "Small Agent is unavailable",
                        false,
                    )
                })?;
            let provider = state
                .providers()
                .iter()
                .find(|provider| provider.provider.id == agent.config.provider_id)
                .ok_or_else(|| {
                    error(
                        ErrorCode::InvalidAgentConfiguration,
                        "Small Agent provider is unavailable",
                        false,
                    )
                })?;
            let title_agent = TitleAgent {
                config: agent.config.clone(),
                provider: provider.provider.clone(),
                credential_ref: state
                    .provider_credentials()
                    .get(&provider.provider.id)
                    .cloned(),
            };
            #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
            let local_only = title_agent.provider.kind == AgentMode::Mock;
            #[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
            let local_only = false;
            if session.title_generation_started || !session.name.trim().is_empty() {
                return Ok((
                    session.clone(),
                    session.workdir,
                    false,
                    local_only,
                    title_agent,
                ));
            }
            if !is_first_completed_interaction(&state, &session) {
                return Err(error(
                    ErrorCode::InvalidSession,
                    "Session has not completed its first interaction",
                    false,
                ));
            }
            state.sessions[index].title_generation_started = true;
            let session = state.sessions[index].clone();
            let event = pending(
                "session.title_generation_started",
                Some(session_id.to_owned()),
                &session,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    return Ok((
                        session.clone(),
                        session.workdir,
                        true,
                        local_only,
                        title_agent,
                    ));
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title update did not settle",
            true,
        ))
    }

    async fn finish_title_generation(
        &self,
        session_id: &str,
        title: String,
        description: String,
    ) -> Result<SessionRecord, ApiError> {
        for _ in 0..4 {
            let loaded = self.records().read_session_record(session_id).await?;
            let mut state = loaded.original.clone();
            let session = state
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            session.title = Some(title.clone());
            session.description.clone_from(&description);
            let session = session.clone();
            let event = pending(
                "session.title_generated",
                Some(session_id.to_owned()),
                &session,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(session),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title update did not settle",
            true,
        ))
    }
}
