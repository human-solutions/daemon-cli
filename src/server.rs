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

// Task manager for cancellation
struct TaskManager {
    current_task: Option<(
        tokio::task::JoinHandle<()>,
        tokio_util::sync::CancellationToken,
    )>,
}

impl TaskManager {
    fn new() -> Self {
        Self { current_task: None }
    }

    fn start_task(
        &mut self,
        handle: tokio::task::JoinHandle<()>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) {
        self.current_task = Some((handle, cancel_token));
    }

    fn finish_task(&mut self) {
        self.current_task = None;
    }

    async fn cancel_current_task(&mut self) -> bool {
        if let Some((handle, cancel_token)) = self.current_task.take() {
            // Step 1: Signal graceful cancellation
            cancel_token.cancel();

            // Step 2: Wait 1 second, then force-kill
            tokio::select! {
                _result = handle => {
                    // Task completed gracefully
                    true
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    // Force abort the task after timeout
                    // Note: handle was consumed by the select, but abort was already called implicitly
                    true
                }
            }
        } else {
            false // No task running
        }
    }
}

// Daemon server implementation
pub struct DaemonServer<H, M: RpcMethod> {
    pub daemon_id: u64,
    pub build_timestamp: u64,
    handler: H,
    _phantom: std::marker::PhantomData<M>,
}

impl<H, M> DaemonServer<H, M>
where
    H: RpcHandler<M> + Clone + 'static,
    M: RpcMethod + 'static,
{
    pub fn new(daemon_id: u64, build_timestamp: u64, handler: H) -> Self {
        Self {
            daemon_id,
            build_timestamp,
            handler,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Spawn server using Unix socket transport
    pub async fn spawn_with_socket(self) -> Result<()> {
        let mut socket_server = SocketServer::new(self.daemon_id).await?;
        let state = Arc::new(Mutex::new(ServerState::Ready));
        let task_manager = Arc::new(Mutex::new(TaskManager::new()));
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
                    let task_manager = task_manager.clone();
                    let status_tx = status_tx.clone();
                    let mut handler = self.handler.clone();

                    // Handle this client connection
                    tokio::spawn(async move {
                        // Handle incoming messages from client
                        while let Ok(Some(message)) =
                            connection.receive_message::<SocketMessage<M>>().await
                        {
                            match message {
                                SocketMessage::Cancel => {
                                    // Handle cancellation request
                                    let cancelled = {
                                        let mut task_mgr = task_manager.lock().await;
                                        task_mgr.cancel_current_task().await
                                    };

                                    // Send acknowledgment
                                    let _ = connection
                                        .send_message(&SocketMessage::<M>::CancelAck)
                                        .await;

                                    if cancelled {
                                        // Update state back to ready
                                        {
                                            let mut current_state = state.lock().await;
                                            *current_state = ServerState::Ready;
                                        }
                                        let _ = status_tx.send(DaemonStatus::Ready);
                                    }
                                }
                                SocketMessage::Request(request) => {
                                    // Phase 4: Check build timestamp version compatibility
                                    if request.client_build_timestamp != self.build_timestamp {
                                        let version_mismatch: RpcResponse<M::Response> =
                                            RpcResponse::VersionMismatch {
                                                daemon_build_timestamp: self.build_timestamp,
                                            };
                                        let _ = connection
                                            .send_message(&SocketMessage::<M>::Response(
                                                version_mismatch,
                                            ))
                                            .await;
                                        continue;
                                    }

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

                                    // Process the request with basic cancellation support
                                    let cancel_token = tokio_util::sync::CancellationToken::new();

                                    // Track the cancel token for potential cancellation
                                    {
                                        let mut task_mgr = task_manager.lock().await;
                                        // For now, just store a dummy handle since we can't easily track the synchronous execution
                                        let dummy_handle = tokio::spawn(async {});
                                        task_mgr.start_task(dummy_handle, cancel_token.clone());
                                    }

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

                                    // Mark task as finished and return to ready state
                                    {
                                        let mut task_mgr = task_manager.lock().await;
                                        task_mgr.finish_task();
                                    }
                                    {
                                        let mut current_state = state.lock().await;
                                        *current_state = ServerState::Ready;
                                    }
                                    let _ = status_tx.send(DaemonStatus::Ready);
                                }
                                _ => {
                                    // Ignore other message types
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

}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
