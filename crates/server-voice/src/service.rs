//! Independently composed speech engines, process budgets and exclusive voice targets.

mod config;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    Error,
    ports::{Agents, Synthesizer, Transcriber},
};

/// Speech services shared across physical connections.
#[derive(Debug, Clone)]
pub struct Speech {
    pub(crate) stt: Option<Arc<dyn Transcriber>>,
    pub(crate) tts: Option<Arc<dyn Synthesizer>>,
    pub(crate) agents: Option<Arc<dyn Agents>>,
    pub(crate) jobs: Arc<Semaphore>,
    streams: Arc<Semaphore>,
    targets: Arc<Mutex<BTreeSet<String>>>,
}

impl Speech {
    /// Compose independent engines and Agent coordination, without I/O.
    #[must_use]
    pub fn new(
        stt: Option<Arc<dyn Transcriber>>,
        tts: Option<Arc<dyn Synthesizer>>,
        agents: Option<Arc<dyn Agents>>,
    ) -> Self {
        Self {
            stt,
            tts,
            agents,
            jobs: Arc::new(Semaphore::new(4)),
            streams: Arc::new(Semaphore::new(16)),
            targets: Arc::default(),
        }
    }

    /// Construct explicitly selected local/HTTP engines from environment variables.
    /// # Errors
    /// Rejects invalid provider selections, missing local models and malformed HTTP settings.
    pub fn from_environment(agents: Option<Arc<dyn Agents>>) -> Result<Self, Error> {
        config::load(|name| std::env::var(name).ok(), agents)
    }

    /// Report whether dictation and full voice conversations are configured.
    #[must_use]
    pub fn availability(&self) -> (bool, bool) {
        (
            self.stt.is_some(),
            self.stt.is_some() && self.tts.is_some() && self.agents.is_some(),
        )
    }

    pub(crate) fn stream(&self) -> Result<OwnedSemaphorePermit, Error> {
        self.streams
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Capacity)
    }

    pub(crate) fn claim(&self, id: String) -> Result<Arc<Target>, Error> {
        let mut targets = self.targets.lock().map_err(|_| Error::Agent)?;
        if !targets.insert(id.clone()) {
            return Err(Error::Capacity);
        }
        Ok(Arc::new(Target {
            id,
            targets: self.targets.clone(),
            lane: tokio::sync::Mutex::new(()),
        }))
    }
}

#[derive(Debug)]
pub(crate) struct Target {
    pub(crate) id: String,
    targets: Arc<Mutex<BTreeSet<String>>>,
    pub(crate) lane: tokio::sync::Mutex<()>,
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Ok(mut targets) = self.targets.lock() {
            targets.remove(&self.id);
        }
    }
}
