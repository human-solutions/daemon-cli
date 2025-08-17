use crate::transport::{SocketClient, SocketMessage, socket_path};
use crate::*;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};
use tokio::process::Command;
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
    daemon_process: Option<tokio::process::Child>,
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
            daemon_process: None, // No process for in-memory connections
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
            daemon_process: None, // Process management handled externally for Phase 2
        })
    }

    // Phase 3: Automatic daemon process spawning
    pub async fn connect(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        // Step 1: Check if daemon is already running
        let socket_path = socket_path(daemon_id);
        
        let (socket_client, daemon_process) = if socket_path.exists() {
            // Try to connect to existing daemon
            match SocketClient::connect(daemon_id).await {
                Ok(client) => {
                    // Daemon is running and connectable
                    (client, None)
                }
                Err(_) => {
                    // Socket exists but daemon is not responding - clean up and restart
                    let _ = std::fs::remove_file(&socket_path);
                    Self::spawn_daemon_and_connect(daemon_id, &daemon_executable).await?
                }
            }
        } else {
            // No socket file - daemon is not running, spawn it
            Self::spawn_daemon_and_connect(daemon_id, &daemon_executable).await?
        };

        let (_status_tx, status_receiver) = broadcast::channel(32);
        let connection = Connection::Socket { socket_client };

        Ok(Self {
            connection,
            status_receiver,
            daemon_id,
            daemon_executable,
            build_timestamp,
            active_cancel_token: None,
            daemon_process,
        })
    }

    async fn spawn_daemon_and_connect(
        daemon_id: u64,
        daemon_executable: &PathBuf,
    ) -> Result<(SocketClient, Option<tokio::process::Child>)> {
        // Spawn daemon process
        let mut child = Command::new(daemon_executable)
            .arg("--daemon-id")
            .arg(daemon_id.to_string())
            .arg("--daemon-mode")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn daemon process: {}", e))?;

        // Wait for daemon to start and socket to become available
        let socket_path = socket_path(daemon_id);
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 50; // 5 seconds total (50 * 100ms)
        
        loop {
            attempts += 1;
            
            // Check if process is still alive
            match child.try_wait() {
                Ok(Some(exit_status)) => {
                    return Err(anyhow::anyhow!("Daemon process exited during startup with status: {}", exit_status));
                }
                Ok(None) => {
                    // Process is still running, continue
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Failed to check daemon process status: {}", e));
                }
            }

            // Try to connect to socket
            if socket_path.exists() {
                match SocketClient::connect(daemon_id).await {
                    Ok(socket_client) => {
                        // Successfully connected
                        return Ok((socket_client, Some(child)));
                    }
                    Err(_) => {
                        // Socket exists but not ready yet, continue waiting
                    }
                }
            }

            if attempts >= MAX_ATTEMPTS {
                // Kill the child process since it's not responding
                let _ = child.kill().await;
                return Err(anyhow::anyhow!("Daemon failed to start within timeout"));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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

    /// Check if daemon process is healthy and restart if needed
    pub async fn ensure_daemon_healthy(&mut self) -> Result<()> {
        // Only applicable for managed processes
        let needs_restart = if let Some(child) = &mut self.daemon_process {
            // Check if process is still alive
            match child.try_wait() {
                Ok(Some(_exit_status)) => {
                    // Process has exited - needs restart
                    true
                }
                Ok(None) => {
                    // Process is alive - try to ping it
                    match Self::ping_daemon_static(self.daemon_id).await {
                        Ok(_) => {
                            // Daemon is responsive
                            false
                        }
                        Err(_) => {
                            // Daemon is not responsive - kill and restart
                            let _ = child.kill().await;
                            true
                        }
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Failed to check daemon process status: {}", e));
                }
            }
        } else {
            false
        };

        if needs_restart {
            self.restart_daemon().await?;
        }

        Ok(())
    }


    async fn ping_daemon_static(daemon_id: u64) -> Result<()> {
        // Try to establish a new connection to test if daemon is responsive
        match SocketClient::connect(daemon_id).await {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Daemon ping failed: {}", e)),
        }
    }

    async fn restart_daemon(&mut self) -> Result<()> {
        // Clean up old socket file
        let socket_path = socket_path(self.daemon_id);
        let _ = std::fs::remove_file(&socket_path);

        // Spawn new daemon process
        let (socket_client, daemon_process) = 
            Self::spawn_daemon_and_connect(self.daemon_id, &self.daemon_executable).await?;

        // Update connection and process handle
        self.connection = Connection::Socket { socket_client };
        self.daemon_process = daemon_process;

        Ok(())
    }

    /// Gracefully shutdown the managed daemon process
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(mut child) = self.daemon_process.take() {
            // Try graceful shutdown first (daemon should exit when no clients)
            tokio::time::sleep(Duration::from_millis(100)).await;
            
            // Check if process exited gracefully
            match child.try_wait() {
                Ok(Some(_)) => {
                    // Process exited gracefully
                }
                Ok(None) => {
                    // Process still running, force kill
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                Err(_) => {
                    // Error checking status, try to kill anyway
                    let _ = child.kill().await;
                }
            }

            // Clean up socket file
            let socket_path = socket_path(self.daemon_id);
            let _ = std::fs::remove_file(&socket_path);
        }
        Ok(())
    }
}

impl<M: RpcMethod> Drop for DaemonClient<M> {
    fn drop(&mut self) {
        // Clean up daemon process on client drop (fire and forget)
        if let Some(mut child) = self.daemon_process.take() {
            tokio::spawn(async move {
                // Try to kill the process
                let _ = child.kill().await;
                let _ = child.wait().await;
            });
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
