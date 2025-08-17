use crate::transport::{SocketMessage, SocketServer};
use crate::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};

// Server state
#[derive(Debug, Clone, PartialEq)]
enum ServerState {
    Ready,
    Busy,
}

// Daemon server implementation
pub struct DaemonServer<H, M: RpcMethod> {
    pub daemon_id: u64,
    handler: H,
    _phantom: std::marker::PhantomData<M>,
}

impl<H, M> DaemonServer<H, M>
where
    H: RpcHandler<M> + Clone + 'static,
    M: RpcMethod + 'static,
{
    pub fn new(daemon_id: u64, handler: H) -> Self {
        Self {
            daemon_id,
            handler,
            _phantom: std::marker::PhantomData,
        }
    }

    pub async fn spawn(self) -> Result<ServerHandle<M>> {
        let (request_tx, mut request_rx) = mpsc::channel::<RequestEnvelope<M>>(32);
        let (status_tx, _status_rx) = broadcast::channel(32);

        let server_handle = ServerHandle {
            request_tx,
            status_tx: status_tx.clone(),
        };

        // Spawn the server task
        tokio::spawn(async move {
            let state = Arc::new(Mutex::new(ServerState::Ready));

            // Send initial status
            let _ = status_tx.send(DaemonStatus::Ready);

            while let Some(envelope) = request_rx.recv().await {
                // Single-task enforcement: reject if busy
                {
                    let current_state = state.lock().await;
                    if *current_state == ServerState::Busy {
                        let _ = envelope.response_tx.send(RpcResponse::Error {
                            error: "Daemon is busy processing another request".to_string(),
                        });
                        continue;
                    }
                }

                // Change state to busy
                {
                    let mut current_state = state.lock().await;
                    *current_state = ServerState::Busy;
                }
                let _ = status_tx.send(DaemonStatus::Busy("Processing request".to_string()));

                // Create status channel for this request
                let (task_status_tx, mut task_status_rx) = mpsc::channel(32);
                let status_tx_clone = status_tx.clone();

                // Forward status updates from handler to broadcast
                let status_forwarder = tokio::spawn(async move {
                    while let Some(status) = task_status_rx.recv().await {
                        let _ = status_tx_clone.send(status);
                    }
                });

                // Process the request in a separate task
                let mut handler = self.handler.clone();
                let state_clone = state.clone();
                let status_tx_for_completion = status_tx.clone();

                tokio::spawn(async move {
                    // Process the request
                    let result = handler
                        .handle(
                            envelope.request.method,
                            envelope.cancel_token,
                            task_status_tx,
                        )
                        .await;

                    // Stop status forwarding
                    status_forwarder.abort();

                    // Send response
                    let response = match result {
                        Ok(output) => RpcResponse::Success { output },
                        Err(err) => RpcResponse::Error {
                            error: err.to_string(),
                        },
                    };

                    let _ = envelope.response_tx.send(response);

                    // Return to ready state
                    {
                        let mut current_state = state_clone.lock().await;
                        *current_state = ServerState::Ready;
                    }
                    let _ = status_tx_for_completion.send(DaemonStatus::Ready);
                });
            }
        });

        Ok(server_handle)
    }

    /// Spawn server using Unix socket transport (Phase 2)
    pub async fn spawn_with_socket(self) -> Result<()> {
        let mut socket_server = SocketServer::new(self.daemon_id).await?;
        let state = Arc::new(Mutex::new(ServerState::Ready));
        let (status_tx, _status_rx) = broadcast::channel::<DaemonStatus>(32);

        // Send initial status
        let _ = status_tx.send(DaemonStatus::Ready);

        let mut client_counter = 0u64;

        println!(
            "Daemon {} listening on socket: {:?}",
            self.daemon_id,
            socket_server.socket_path()
        );

        loop {
            match socket_server.accept().await {
                Ok(mut connection) => {
                    let client_id = client_counter;
                    client_counter += 1;

                    let state = state.clone();
                    let status_tx = status_tx.clone();
                    let mut handler = self.handler.clone();

                    // Handle this client connection
                    tokio::spawn(async move {
                        // Handle incoming messages from client
                        while let Ok(Some(message)) =
                            connection.receive_message::<SocketMessage<M>>().await
                        {
                            match message {
                                SocketMessage::Request(request) => {
                                    // Single-task enforcement: reject if busy
                                    {
                                        let current_state = state.lock().await;
                                        if *current_state == ServerState::Busy {
                                            let error_response: RpcResponse<M::Response> =
                                                RpcResponse::Error {
                                                    error:
                                                        "Daemon is busy processing another request"
                                                            .to_string(),
                                                };
                                            let _ = connection
                                                .send_message(&SocketMessage::<M>::Response(
                                                    error_response,
                                                ))
                                                .await;
                                            continue;
                                        }
                                    }

                                    // Change state to busy
                                    {
                                        let mut current_state = state.lock().await;
                                        *current_state = ServerState::Busy;
                                    }
                                    let _ = status_tx
                                        .send(DaemonStatus::Busy("Processing request".to_string()));

                                    // Create status channel for this request
                                    let (task_status_tx, mut task_status_rx) = mpsc::channel(32);
                                    let status_tx_clone = status_tx.clone();

                                    // Forward task status updates to broadcast
                                    let task_status_forwarder = tokio::spawn(async move {
                                        while let Some(status) = task_status_rx.recv().await {
                                            let _ = status_tx_clone.send(status);
                                        }
                                    });

                                    // Process the request synchronously for simplicity in Phase 2
                                    let cancel_token = tokio_util::sync::CancellationToken::new();

                                    let result = handler
                                        .handle(request.method, cancel_token, task_status_tx)
                                        .await;

                                    // Stop status forwarding
                                    task_status_forwarder.abort();

                                    // Send response
                                    let response = match result {
                                        Ok(output) => RpcResponse::Success { output },
                                        Err(err) => RpcResponse::Error {
                                            error: err.to_string(),
                                        },
                                    };

                                    let _ = connection
                                        .send_message(&SocketMessage::<M>::Response(response))
                                        .await;

                                    // Return to ready state
                                    {
                                        let mut current_state = state.lock().await;
                                        *current_state = ServerState::Ready;
                                    }
                                    let _ = status_tx.send(DaemonStatus::Ready);
                                }
                                SocketMessage::Subscribe => {
                                    // Client wants to subscribe to status updates (already handled by status_forwarder)
                                }
                                _ => {
                                    // Ignore other message types for now
                                }
                            }
                        }

                        // Clean up when client disconnects
                        println!("Client {client_id} disconnected");
                    });
                }
                Err(e) => {
                    eprintln!("Failed to accept connection: {e}");
                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        todo!("Implement daemon server main loop - use spawn() instead for Phase 1")
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
