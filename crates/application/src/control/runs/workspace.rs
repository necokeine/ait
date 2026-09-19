//! Supervision for API Runs and the development-only mock provider.
use super::{RunRecord, finalization::RunControl};
use crate::control::{
    LocalControlService,
    errors::{api_domain_error, error},
};
use ait_contracts::{AgentMode, ApiError};
use ait_domain::{DomainError, ErrorCode};
use std::sync::Arc;

impl LocalControlService {
    async fn drive_run(
        &self,
        run_id: &str,
        control: Arc<RunControl>,
    ) -> Result<RunRecord, ApiError> {
        let state = self.read_run_records(run_id).await?.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "Run not found", false))?;
        match run.provider.kind {
            AgentMode::OpenAI | AgentMode::DeepSeek | AgentMode::Gemini | AgentMode::MiniMax => {
                self.execute_api_run(run, control.cancellation.clone())
                    .await
            }
            AgentMode::Codex => Err(error(
                ErrorCode::CodexThreadCapabilityUnsupported,
                "Codex requires a prepared persistent native Thread",
                false,
            )),
            #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
            AgentMode::Mock => {
                self.finish_execution(run_id, Ok(Some("Mock assistant response.".into())))
                    .await
            }
        }
    }

    pub(in crate::control) async fn supervise_run(
        &self,
        run_id: String,
        control: Arc<RunControl>,
    ) -> Result<RunRecord, ApiError> {
        let worker = self.clone();
        let id = run_id.clone();
        let result =
            tokio::spawn(async move { Box::pin(worker.drive_run(&id, control)).await }).await;
        match result {
            Ok(Ok(run)) => Ok(run),
            Ok(Err(failure)) => {
                self.finish_execution(&run_id, Err(api_domain_error(failure)))
                    .await
            }
            Err(_) => {
                self.finish_execution(
                    &run_id,
                    Err(DomainError::invariant(
                        ErrorCode::ProviderFailed,
                        "Run executor stopped before settlement",
                    )),
                )
                .await
            }
        }
    }
}
