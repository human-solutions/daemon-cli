use crate::transport::{SocketClient, SocketMessage};
use crate::*;
use anyhow::Result;
use tokio::sync::oneshot;
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