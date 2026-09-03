use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream;

use crate::{
    ProviderAdapter, ProviderCapabilities, ProviderError, ProviderEvent, ProviderInvocation,
    ProviderStream, validate_request,
};

/// Deterministic local provider used for offline runs and contract tests.
#[derive(Debug, Clone)]
pub struct ScriptedProvider {
    capabilities: ProviderCapabilities,
    script: Vec<Result<ProviderEvent, ProviderError>>,
    delay: Duration,
}

impl ScriptedProvider {
    #[must_use]
    pub fn new(
        capabilities: ProviderCapabilities,
        script: Vec<Result<ProviderEvent, ProviderError>>,
    ) -> Self {
        Self {
            capabilities,
            script,
            delay: Duration::ZERO,
        }
    }

    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedProvider {
    fn driver(&self) -> &'static str {
        "mock"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    async fn stream(
        &self,
        invocation: ProviderInvocation,
    ) -> Result<ProviderStream, ProviderError> {
        validate_request(&invocation.request, self.capabilities)?;
        let events = self.script.clone();
        let cancellation = invocation.cancellation;
        let delay = self.delay;
        Ok(Box::pin(stream::unfold(
            (events.into_iter(), cancellation, delay),
            |(mut events, cancellation, delay)| async move {
                if cancellation.is_cancelled() {
                    return Some((
                        Err(ProviderError::cancelled()),
                        (events, cancellation, delay),
                    ));
                }
                let event = events.next()?;
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                Some((event, (events, cancellation, delay)))
            },
        )))
    }
}
