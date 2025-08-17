use crate::transport::{SocketClient, SocketMessage, socket_path};
use crate::*;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::broadcast;

// Daemon client for communicating with daemon
pub struct DaemonClient<M: RpcMethod> {
    socket_client: SocketClient,
    pub status_receiver: broadcast::Receiver<DaemonStatus>,
    pub daemon_id: u64,
    pub daemon_executable: PathBuf,
    pub build_timestamp: u64,
    daemon_process: Option<tokio::process::Child>,
    _phantom: std::marker::PhantomData<M>,
}

impl<M: RpcMethod> DaemonClient<M> {
    // Connect to existing daemon via socket
    pub async fn connect_via_socket(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        let socket_client = SocketClient::connect(daemon_id).await?;
        let (_status_tx, status_receiver) = broadcast::channel(32);

        Ok(Self {
            socket_client,
            status_receiver,
            daemon_id,
            daemon_executable,
            build_timestamp,
            daemon_process: None, // Process management handled externally
            _phantom: std::marker::PhantomData,
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
                Ok(_client) => {
                    // Daemon is running but we don't manage it - we should still be able to restart if needed
                    // For Phase 4, we always spawn our own daemon to have full control
                    let _ = std::fs::remove_file(&socket_path);

                    // Kill any existing daemon processes (best effort)
                    Self::kill_existing_daemon_processes(daemon_id).await;

                    // Spawn our own daemon
                    Self::spawn_daemon_and_connect(daemon_id, &daemon_executable, build_timestamp)
                        .await?
                }
                Err(_) => {
                    // Socket exists but daemon is not responding - clean up and restart
                    let _ = std::fs::remove_file(&socket_path);
                    Self::spawn_daemon_and_connect(daemon_id, &daemon_executable, build_timestamp)
                        .await?
                }
            }
        } else {
            // No socket file - daemon is not running, spawn it
            Self::spawn_daemon_and_connect(daemon_id, &daemon_executable, build_timestamp).await?
        };

        let (_status_tx, status_receiver) = broadcast::channel(32);

        Ok(Self {
            socket_client,
            status_receiver,
            daemon_id,
            daemon_executable,
            build_timestamp,
            daemon_process,
            _phantom: std::marker::PhantomData,
        })
    }

    async fn spawn_daemon_and_connect(
        daemon_id: u64,
        daemon_executable: &PathBuf,
        build_timestamp: u64,
    ) -> Result<(SocketClient, Option<tokio::process::Child>)> {
        // Spawn daemon process
        let mut child = Command::new(daemon_executable)
            .arg("daemon")
            .arg("--daemon-id")
            .arg(daemon_id.to_string())
            .arg("--build-timestamp")
            .arg(build_timestamp.to_string())
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
                    return Err(anyhow::anyhow!(
                        "Daemon process exited during startup with status: {}",
                        exit_status
                    ));
                }
                Ok(None) => {
                    // Process is still running, continue
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to check daemon process status: {}",
                        e
                    ));
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

    async fn kill_existing_daemon_processes(daemon_id: u64) {
        // Best effort to kill existing daemon processes
        // This is a simplified implementation - in production you'd want more sophisticated process management
        let _ = tokio::process::Command::new("pkill")
            .arg("-f")
            .arg(format!("all_in_one.*daemon.*--daemon-id.*{daemon_id}"))
            .output()
            .await;

        // Give time for processes to die
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    pub async fn request(&mut self, method: M) -> Result<RpcResponse<M::Response>> {
        let request = RpcRequest {
            method: method.clone(),
            client_build_timestamp: self.build_timestamp,
        };

        // Send request over socket
        self.socket_client
            .send_message(&SocketMessage::Request(request))
            .await?;

        // Wait for response
        let response = if let Some(message) = self
            .socket_client
            .receive_message::<SocketMessage<M>>()
            .await?
        {
            match message {
                SocketMessage::Response(response) => response,
                _ => return Err(anyhow::anyhow!("Unexpected message type")),
            }
        } else {
            return Err(anyhow::anyhow!("Socket connection closed"));
        };

        // Phase 4: Handle version mismatch by restarting daemon
        if let RpcResponse::VersionMismatch {
            daemon_build_timestamp,
        } = &response
        {
            println!(
                "Version mismatch detected: client={}, daemon={}. Restarting daemon...",
                self.build_timestamp, daemon_build_timestamp
            );

            // Only restart if we manage the daemon process
            if self.daemon_process.is_some() {
                self.restart_daemon().await?;

                // Retry the request with the new daemon
                let retry_request = RpcRequest {
                    method: method.clone(),
                    client_build_timestamp: self.build_timestamp,
                };

                // Send retry request over socket
                self.socket_client
                    .send_message(&SocketMessage::Request(retry_request))
                    .await?;

                // Wait for retry response
                let retry_response = if let Some(message) = self
                    .socket_client
                    .receive_message::<SocketMessage<M>>()
                    .await?
                {
                    match message {
                        SocketMessage::Response(response) => response,
                        _ => return Err(anyhow::anyhow!("Unexpected message type")),
                    }
                } else {
                    return Err(anyhow::anyhow!("Socket connection closed"));
                };

                return Ok(retry_response);
            }
        }

        Ok(response)
    }

    /// Cancel the currently running task on the daemon
    pub async fn cancel_current_task(&mut self) -> Result<bool> {
        // Send cancel message
        self.socket_client
            .send_message(&SocketMessage::<M>::Cancel)
            .await?;

        // Wait for acknowledgment (with timeout)
        tokio::select! {
            msg = self.socket_client.receive_message::<SocketMessage<M>>() => {
                match msg? {
                    Some(SocketMessage::CancelAck) => Ok(true),
                    _ => Ok(false),
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                Ok(false) // Timeout, assume failed
            }
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
                    return Err(anyhow::anyhow!(
                        "Failed to check daemon process status: {}",
                        e
                    ));
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
        let (socket_client, daemon_process) = Self::spawn_daemon_and_connect(
            self.daemon_id,
            &self.daemon_executable,
            self.build_timestamp,
        )
        .await?;

        // Update socket client and process handle
        self.socket_client = socket_client;
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
