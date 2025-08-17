use crate::transport::{SocketClient, SocketMessage};
use crate::*;
use anyhow::Result;
use std::path::PathBuf;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

// Connection abstraction for different transport types
pub enum Connection<M: RpcMethod> {
    InMemory {
        server_handle: ServerHandle<M>,
    },
    Socket {
        socket_client: SocketClient,
    },
}

impl<M: RpcMethod> Connection<M> {
    pub async fn send_request(
        &mut self,
        request: RpcRequest<M>,
        cancel_token: CancellationToken,
    ) -> Result<RpcResponse<M::Response>> {
        match self {
            Connection::InMemory { server_handle } => {
                // Phase 1: In-memory communication
                let (response_tx, response_rx) = oneshot::channel();
                let envelope = RequestEnvelope {
                    request,
                    response_tx,
                    cancel_token,
                };

                // Send request to server
                server_handle
                    .request_tx
                    .send(envelope)
                    .await
                    .map_err(|_| anyhow::anyhow!("Server is not running"))?;

                // Wait for response
                response_rx
                    .await
                    .map_err(|_| anyhow::anyhow!("Server did not respond"))
            }
            Connection::Socket { socket_client } => {
                // Phase 2: Unix socket communication
                
                // Send request over socket
                socket_client
                    .send_message(&SocketMessage::Request(request))
                    .await?;

                // Wait for response
                if let Some(message) = socket_client.receive_message::<SocketMessage<M>>().await? {
                    match message {
                        SocketMessage::Response(response) => Ok(response),
                        _ => Err(anyhow::anyhow!("Unexpected message type")),
                    }
                } else {
                    Err(anyhow::anyhow!("Socket connection closed"))
                }
            }
        }
    }
}

// Daemon client for communicating with daemon
pub struct DaemonClient<M: RpcMethod> {
    connection: Connection<M>,
    pub status_receiver: broadcast::Receiver<DaemonStatus>,
    pub daemon_id: u64,
    pub daemon_executable: PathBuf,
    pub build_timestamp: u64,
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
        let connection = Connection::InMemory {
            server_handle: ServerHandle {
                request_tx: server_handle.request_tx.clone(),
                status_tx: server_handle.status_tx.clone(),
            },
        };

        Self {
            connection,
            status_receiver: server_handle.subscribe_status(),
            daemon_id,
            daemon_executable,
            build_timestamp,
            active_cancel_token: None,
        }
    }

    // For Phase 2: connect using Unix socket
    pub async fn connect_via_socket(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        let socket_client = SocketClient::connect(daemon_id).await?;
        let (_status_tx, status_receiver) = broadcast::channel(32);

        let connection = Connection::Socket { socket_client };

        Ok(Self {
            connection,
            status_receiver,
            daemon_id,
            daemon_executable,
            build_timestamp,
            active_cancel_token: None,
        })
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
        let cancel_token = CancellationToken::new();
        self.active_cancel_token = Some(cancel_token.clone());

        let request = RpcRequest {
            method,
            client_build_timestamp: self.build_timestamp,
        };

        let response = self.connection.send_request(request, cancel_token).await?;

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
