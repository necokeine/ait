use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use server_api::Api;
use server_application::Projects;
use server_ports::{ProjectError, ProjectStorage, ProjectStore};
use server_storage::{SqliteCatalog, SqliteProjects};
use server_workspace::LocalWorkspace;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::instance::InstanceLease;

pub(super) struct Server {
    listener: TcpListener,
    api: Api,
    // Own the directory lock until all accepted connections have stopped.
    instance: Arc<InstanceLease>,
}

#[derive(Debug)]
struct OwnedStorage {
    // A timed-out or abandoned response must not release the catalog's process lease
    // while its supervised blocking job can still write. Projects owns this factory
    // until after its open stores and catalog are dropped.
    _instance: Arc<InstanceLease>,
}

impl ProjectStorage for OwnedStorage {
    fn open(&self, root: &Path) -> Result<Box<dyn ProjectStore>, ProjectError> {
        SqliteProjects.open(root)
    }
}

impl Server {
    pub async fn bind(config: Config) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(config.listen)
            .await
            .context("bind server listener")?;
        let address = listener
            .local_addr()
            .context("read server listener address")?;
        let (instance, projects) = tokio::task::spawn_blocking(move || {
            let instance = Arc::new(InstanceLease::acquire(&config.data_dir)?);
            let catalog = SqliteCatalog::open(&config.data_dir)?;
            let workspace = LocalWorkspace::for_user()?;
            let projects = Projects::new(
                Box::new(catalog),
                Box::new(OwnedStorage {
                    _instance: instance.clone(),
                }),
                Box::new(workspace),
            );
            Ok::<_, anyhow::Error>((instance, projects))
        })
        .await
        .context("join server initialization")??;
        let api = Api::new(
            address,
            instance.server_id.to_string(),
            instance.instance_id.to_string(),
            config.token,
            Some(projects),
        )?;
        Ok(Self {
            listener,
            api,
            instance,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound TCP listener has a local address")
    }

    pub async fn serve(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let Self {
            listener,
            api,
            instance,
        } = self;
        let result = async {
            let shutdown_api = api.clone();
            let server = axum::serve(listener, api.router())
                .with_graceful_shutdown(async move {
                    shutdown.await;
                    shutdown_api.begin_shutdown();
                })
                .into_future();
            tokio::pin!(server);
            // Readiness changes in the signal future before HTTP acceptance stops.
            // Also clean up WS tasks if the HTTP server terminates with an error.
            let result = tokio::select! {
                result = &mut server => Some(result),
                () = api.wait_draining() => None,
            };
            api.begin_shutdown();
            tokio::time::timeout(Duration::from_secs(15), async {
                let result = match result {
                    Some(result) => result,
                    None => server.await,
                };
                api.wait_closed().await;
                result.context("serve HTTP")
            })
            .await
            .context("server shutdown exceeded 15 seconds")??;
            Ok(())
        }
        .await;
        // Drop routers and application storage before the data-directory instance lease.
        drop(api);
        drop(instance);
        result
    }
}

#[cfg(test)]
mod tests;
