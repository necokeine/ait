use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use server_api::Api;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::instance::InstanceLease;

pub(super) struct Server {
    listener: TcpListener,
    api: Api,
    // Own the directory lock until all accepted connections have stopped.
    instance: InstanceLease,
}

impl Server {
    pub async fn bind(config: Config) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(config.listen)
            .await
            .context("bind server listener")?;
        let address = listener
            .local_addr()
            .context("read server listener address")?;
        let instance =
            tokio::task::spawn_blocking(move || InstanceLease::acquire(&config.data_dir))
                .await
                .context("join server initialization")??;
        let api = Api::new(
            address,
            instance.server_id.to_string(),
            instance.instance_id.to_string(),
            config.token,
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
        drop(instance);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
