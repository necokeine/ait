//! API Run execution orchestration over the provider-neutral `RunCoordinator`.
mod agent;
mod store;
mod terminal;

use std::sync::Arc;

use ait_contracts::ApiError;
use ait_domain::{DomainError, ErrorCode, RunId};
use ait_ports::{RunStore, RunTool};
use ait_runtime::{RunCoordinator, SystemClock, UuidIds};
use futures_util::FutureExt;

use crate::control::LocalControlService;
use crate::control::errors::{error, recovery_error};
use crate::control::events::now;
use crate::control::permissions::validate_run_permission_ceiling;
use crate::control::persistence::{HasProjects, HasRunCredentials, HasSessions};
use crate::control::project::worktrees::run_workdir;
use crate::control::runs::{RunRecord, is_terminal_run_status};
use agent::{NoTools, ProviderAgent};
use store::ControlRunStore;

pub(in crate::control) use terminal::{append_terminal_results, interrupt, needs_terminal_repair};

impl LocalControlService {
    #[allow(
        clippy::too_many_lines,
        reason = "execution and failure settlement remain a single guarded lifecycle"
    )]
    pub(in crate::control) async fn execute_api_run(
        &self,
        view: &RunRecord,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<RunRecord, ApiError> {
        if is_terminal_run_status(view.status()) {
            return Ok(view.clone());
        }
        if view.status() == ait_domain::LifecycleStatus::Cancelling {
            cancellation.cancel();
        }
        let store = Arc::new(ControlRunStore::new(self.clone(), view.id.clone()));
        store
            .initialize(view)
            .await
            .map_err(|_| recovery_error("could not initialize API Run"))?;
        let current = store.latest_view().await?;
        if is_terminal_run_status(current.status()) {
            return Ok(current);
        }
        // A recovered durable cancel is a settlement instruction, never a new
        // provider/tool invocation (even if credentials or the Project moved).
        if current.status() == ait_domain::LifecycleStatus::Cancelling {
            let mut run = store
                .load_run(&RunId::new(&view.id))
                .await
                .map_err(|_| recovery_error("could not read cancelled API Run"))?;
            run.stop(
                ait_domain::RunStopReason::Cancelled,
                ait_domain::TimestampMs(now()),
                run.error.clone(),
            )
            .map_err(api_domain_error)?;
            store
                .save_run(run)
                .await
                .map_err(|_| recovery_error("could not settle cancelled API Run"))?;
            return store.latest_view().await;
        }
        let state = self.read_run_records(&view.id).await?.original;
        if let Some(dispatcher) = &self.run_dispatcher {
            validate_run_permission_ceiling(view.permission_profile, self.permission_limits)
                .map_err(api_domain_error)?;
            let workdir = run_workdir(&state, view)?;
            let reference = state
                .run_credentials
                .get(&view.id)
                .ok_or_else(|| recovery_error("Run credential reference is missing"))?;
            let credential = self
                .provider_gateway
                .as_ref()
                .ok_or_else(|| recovery_error("provider gateway is unavailable"))?
                .credential_grant(reference)
                .await
                .map_err(api_domain_error)?;
            dispatcher
                .dispatch(ait_ports::ApiRunDispatch {
                    run_id: RunId::new(&view.id),
                    workdir,
                    permission: view.permission_profile,
                    maximum_sandbox: self.permission_limits.max_sandbox,
                    provider: view.provider.clone(),
                    config: view.config.clone(),
                    credential,
                    store: store.clone(),
                    cancellation,
                })
                .await
                .map_err(api_domain_error)?;
            return store.latest_view().await;
        }
        let (tools, agent) = match self.prepare_api_executor(view, &state).await {
            Ok(parts) => parts,
            Err(failure) => {
                let mut run = store
                    .load_run(&RunId::new(&view.id))
                    .await
                    .map_err(|_| recovery_error("could not load API Run"))?;
                run.stop(
                    ait_domain::RunStopReason::Failed,
                    ait_domain::TimestampMs(now()),
                    Some(failure),
                )
                .map_err(api_domain_error)?;
                store
                    .save_run(run)
                    .await
                    .map_err(|_| recovery_error("could not persist API Run rejection"))?;
                return store.latest_view().await;
            }
        };
        let coordinator = RunCoordinator::new(
            store.clone(),
            Arc::new(agent),
            tools.clone(),
            Arc::new(self.clone()),
            Arc::new(SystemClock),
            Arc::new(UuidIds),
        );
        let outcome =
            std::panic::AssertUnwindSafe(coordinator.drive(&RunId::new(&view.id), cancellation))
                .catch_unwind()
                .await;
        // Includes panic/store-error paths: a dropped tool future can still own
        // blocking I/O. Join it before the outer supervisor writes terminal state.
        tools.cancel_and_drain().await;
        outcome
            .map_err(|_| recovery_error("API execution task failed; effects require review"))?
            .map_err(|_| {
                recovery_error(
                    "API Run stopped at a durable boundary; inspect execution before recovery",
                )
            })?;
        store.latest_view().await
    }

    async fn prepare_api_executor(
        &self,
        view: &RunRecord,
        state: &(impl HasProjects + HasRunCredentials + HasSessions),
    ) -> Result<(Arc<dyn RunTool>, ProviderAgent), DomainError> {
        validate_run_permission_ceiling(view.permission_profile, self.permission_limits)?;
        let root = run_workdir(state, view).map_err(|failure| DomainError {
            code: failure.code,
            message: failure.message,
            retryable: failure.retryable,
            details: None,
            cause_id: None,
        })?;
        let primary_tools = match &self.api_tools {
            Some(factory) => {
                let factory = factory.clone();
                let profile = view.permission_profile;
                tokio::task::spawn_blocking(move || factory.create(&root, profile))
                    .await
                    .map_err(|_| {
                        DomainError::invariant(
                            ErrorCode::ToolExecutionFailed,
                            "host executor preparation failed",
                        )
                    })??
            }
            None => Arc::new(NoTools) as Arc<dyn RunTool>,
        };
        let gateway = self.provider_gateway.clone().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "provider gateway is unavailable",
            )
        })?;
        let credential = state
            .run_credentials()
            .get(&view.id)
            .cloned()
            .ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::InvalidConfiguration,
                    "provider credential is missing",
                )
            })?;
        let child_agent = ProviderAgent {
            gateway: gateway.clone(),
            view: view.clone(),
            credential: credential.clone(),
            names: primary_tools.executable_tools(),
        };
        let tools = self.api_tools.as_ref().map_or_else(
            || primary_tools.clone(),
            |factory| {
                factory.extend_agent_tools(
                    primary_tools.clone(),
                    Arc::new(child_agent),
                    Arc::new(self.clone()),
                )
            },
        );
        let agent = ProviderAgent {
            gateway,
            view: view.clone(),
            credential,
            names: tools.executable_tools(),
        };
        Ok((tools, agent))
    }
}

fn api_domain_error(domain_error: DomainError) -> ApiError {
    error(
        domain_error.code,
        domain_error.message,
        domain_error.retryable,
    )
}
