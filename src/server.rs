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

    pub async fn run(&mut self) -> Result<()> {
        todo!("Implement daemon server main loop - use spawn() instead for Phase 1")
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
