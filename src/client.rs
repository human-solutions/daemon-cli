use crate::*;
use anyhow::Result;
use std::path::PathBuf;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

// Daemon client for communicating with daemon
pub struct DaemonClient<M: RpcMethod> {
    pub status_receiver: broadcast::Receiver<DaemonStatus>,
    pub daemon_id: u64,
    pub daemon_executable: PathBuf,
    pub build_timestamp: u64,
    server_handle: ServerHandle<M>,
    active_cancel_token: Option<CancellationToken>,
}

impl<M: RpcMethod> DaemonClient<M> {
    // For Phase 1: connect using a server handle (in-memory)
    pub fn connect_to_server(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
        server_handle: &ServerHandle<M>,
    ) -> Self {
        Self {
            status_receiver: server_handle.subscribe_status(),
            daemon_id,
            daemon_executable,
            build_timestamp,
            server_handle: ServerHandle {
                request_tx: server_handle.request_tx.clone(),
                status_tx: server_handle.status_tx.clone(),
            },
            active_cancel_token: None,
        }
    }

    // Original connect method (for future phases)
    pub async fn connect(
        _daemon_id: u64,
        _daemon_executable: PathBuf,
        _build_timestamp: u64,
    ) -> Result<Self> {
        todo!("Implement daemon process spawning in Phase 3")
    }

    pub async fn request(&mut self, method: M) -> Result<RpcResponse<M::Response>> {
        let (response_tx, response_rx) = oneshot::channel();
        let cancel_token = CancellationToken::new();
        self.active_cancel_token = Some(cancel_token.clone());

        let request = RpcRequest {
            method,
            client_build_timestamp: self.build_timestamp,
        };

        let envelope = RequestEnvelope {
            request,
            response_tx,
            cancel_token,
        };

        // Send request to server
        self.server_handle
            .request_tx
            .send(envelope)
            .await
            .map_err(|_| anyhow::anyhow!("Server is not running"))?;

        // Wait for response
        let response = response_rx
            .await
            .map_err(|_| anyhow::anyhow!("Server did not respond"))?;

        self.active_cancel_token = None;
        Ok(response)
    }

    pub async fn cancel(&mut self) -> Result<()> {
        if let Some(cancel_token) = &self.active_cancel_token {
            cancel_token.cancel();
            Ok(())
        } else {
            Err(anyhow::anyhow!("No active request to cancel"))
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
