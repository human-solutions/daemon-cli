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
    /// Zero-configuration daemon connection - spawns daemon if needed
    pub async fn connect(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        // Try to connect to existing daemon first
        let socket_path = socket_path(daemon_id);

        let (socket_client, daemon_process) = if let Ok(existing_client) =
            SocketClient::connect(daemon_id).await
        {
            // Daemon is already running and responsive - use it
            (existing_client, None) // We don't manage existing daemons
        } else {
            // Daemon not running or not responsive - spawn our own
            if socket_path.exists() {
                // Clean up stale socket file
                let _ = std::fs::remove_file(&socket_path);
            }

            // Kill any zombie processes (best effort)
            Self::cleanup_stale_processes(daemon_id).await;

            // Spawn new daemon
            Self::spawn_and_wait_for_ready(daemon_id, &daemon_executable, build_timestamp).await?
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

    async fn spawn_and_wait_for_ready(
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

        // Wait for daemon to become ready
        let socket_path = socket_path(daemon_id);
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 50; // 5 seconds total

        loop {
            attempts += 1;

            // Check if process crashed
            if let Ok(Some(exit_status)) = child.try_wait() {
                return Err(anyhow::anyhow!(
                    "Daemon process exited during startup with status: {}",
                    exit_status
                ));
            }

            // Try to connect
            if socket_path.exists() {
                if let Ok(socket_client) = SocketClient::connect(daemon_id).await {
                    // Successfully connected
                    return Ok((socket_client, Some(child)));
                }
            }

            if attempts >= MAX_ATTEMPTS {
                let _ = child.kill().await;
                return Err(anyhow::anyhow!("Daemon failed to start within timeout"));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn cleanup_stale_processes(daemon_id: u64) {
        // Best effort cleanup of stale daemon processes
        let _ = tokio::process::Command::new("pkill")
            .arg("-f")
            .arg(format!("cli.*daemon.*--daemon-id.*{daemon_id}"))
            .output()
            .await;

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

    async fn restart_daemon(&mut self) -> Result<()> {
        // Clean up old socket file
        let socket_path = socket_path(self.daemon_id);
        let _ = std::fs::remove_file(&socket_path);

        // Spawn new daemon process
        let (socket_client, daemon_process) = Self::spawn_and_wait_for_ready(
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
