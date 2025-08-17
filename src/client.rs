use crate::transport::{SocketClient, SocketMessage, socket_path};
use crate::*;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::broadcast;

/// Client for communicating with daemon processes via Unix sockets.
///
/// Provides zero-configuration daemon management with automatic spawning,
/// version synchronization, and process lifecycle management.
///
/// # Example
///
/// ```rust
/// use daemon_rpc::prelude::*;
/// use std::path::PathBuf;
///
/// #[derive(Serialize, Deserialize, Debug, Clone)]
/// enum MyMethod {
///     Process { data: String },
///     GetStatus,
/// }
///
/// impl RpcMethod for MyMethod {
///     type Response = String;
/// }
///
/// // Demonstrate the API structure and types
/// let method = MyMethod::Process { data: "hello".to_string() };
/// let daemon_exe = PathBuf::from("./target/debug/examples/cli");
/// let build_timestamp = 1234567890u64;
///
/// // Verify method serialization works
/// let json = serde_json::to_string(&method).unwrap();
/// assert!(json.contains("hello"));
///
/// // Show response handling pattern
/// let response: RpcResponse<String> = RpcResponse::Success {
///     output: "Task completed".to_string()
/// };
///
/// match response {
///     RpcResponse::Success { output } => {
///         assert_eq!(output, "Task completed");
///     }
///     RpcResponse::Error { error } => {
///         panic!("Unexpected error: {}", error);
///     }
///     _ => panic!("Unexpected response type"),
/// }
///
/// // Actual usage pattern (requires daemon binary):
///  # tokio_test::block_on(async {
///      let Ok(mut client) = DaemonClient::connect(1000, daemon_exe, build_timestamp).await else {
///         // handle error...
///         return;
///      };
///      let response = client.request(method).await;
///      // Handle response...
///      let _ = client.shutdown().await;
///  # });
/// ```
pub struct DaemonClient<M: RpcMethod> {
    socket_client: SocketClient,
    /// Stream of real-time status updates from the daemon
    pub status_receiver: broadcast::Receiver<DaemonStatus>,
    /// Unique identifier for this daemon instance
    pub daemon_id: u64,
    /// Path to the daemon executable for spawning
    pub daemon_executable: PathBuf,
    /// Build timestamp for version compatibility checking
    pub build_timestamp: u64,
    daemon_process: Option<tokio::process::Child>,
    _phantom: std::marker::PhantomData<M>,
}

impl<M: RpcMethod> DaemonClient<M> {
    /// Connect to a daemon with zero configuration - automatically spawns if needed.
    ///
    /// This is the primary method for establishing daemon connections. The framework
    /// automatically handles:
    /// - Detecting if daemon is already running
    /// - Spawning new daemon process if needed
    /// - Waiting for daemon to become ready
    /// - Establishing Unix socket connection
    /// - Managing daemon process lifecycle
    ///
    /// # Parameters
    ///
    /// * `daemon_id` - Unique identifier for the daemon instance
    /// * `daemon_executable` - Path to the daemon binary to spawn if needed
    /// * `build_timestamp` - Build timestamp for version compatibility checking
    ///
    /// # Returns
    ///
    /// A connected client ready to send RPC requests
    ///
    /// # Example
    ///
    /// ```rust
    /// use daemon_rpc::prelude::*;
    /// use std::path::PathBuf;
    ///
    /// // For doc test - shows API structure without requiring actual daemon binary
    /// let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    ///
    /// // Demonstrates the connect API (would spawn daemon if binary existed)
    /// # tokio_test::block_on(async {
    ///     // Define types for the doc test
    ///     use daemon_rpc::prelude::*;
    ///     #[derive(Serialize, Deserialize, Debug, Clone)]
    ///     enum TestMethod { GetStatus }
    ///     impl RpcMethod for TestMethod { type Response = String; }
    ///
    ///     let Ok(mut _client): Result<DaemonClient<TestMethod>> = DaemonClient::connect(1000, daemon_exe.clone(), 1234567890).await else {
    ///         return; // Skip if daemon binary doesn't exist
    ///     };
    ///     let _ = _client.shutdown().await;
    /// # });
    ///
    /// // API usage pattern:
    /// assert_eq!(daemon_exe.to_string_lossy().contains("cli"), true);
    /// ```
    ///
    /// # Behavior
    ///
    /// - If daemon is running and responsive → connects to existing daemon
    /// - If daemon is not running → spawns new daemon process
    /// - If daemon is unresponsive → cleans up and spawns new daemon
    /// - Waits up to 5 seconds for daemon to become ready
    /// - Returns error if daemon fails to start
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
            if socket_path.exists()
                && let Ok(socket_client) = SocketClient::connect(daemon_id).await {
                    // Successfully connected
                    return Ok((socket_client, Some(child)));
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

    /// Send an RPC request to the daemon and wait for response.
    ///
    /// Automatically handles version checking and daemon restart if needed.
    /// The daemon processes one request at a time, so concurrent requests
    /// will be rejected with a busy error.
    ///
    /// # Parameters
    ///
    /// * `method` - The RPC method to execute on the daemon
    ///
    /// # Returns
    ///
    /// The response from the daemon method execution
    ///
    /// # Behavior
    ///
    /// - **Version Checking**: Compares build timestamps and restarts daemon if mismatched
    /// - **Error Handling**: Propagates daemon errors with clear messages
    /// - **Connection Recovery**: Detects broken connections and attempts recovery
    pub async fn request(&mut self, method: M) -> Result<RpcResponse<M::Response>>
    where
        M: Clone, // Required for automatic retry on version mismatch
    {
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

    /// Cancel the currently running task on the daemon.
    ///
    /// Sends a cancellation message to the daemon, which will attempt to
    /// gracefully stop the current task. If the task doesn't respond within
    /// 1 second, it will be forcefully terminated.
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Task was cancelled successfully
    /// * `Ok(false)` - No task was running or cancellation failed
    /// * `Err(_)` - Communication error with daemon
    ///
    /// # Behavior
    ///
    /// 1. Sends cancel message to daemon
    /// 2. Daemon signals running task to stop gracefully
    /// 3. After 1 second timeout, daemon force-kills the task
    /// 4. Daemon sends acknowledgment back to client
    /// 5. Returns success/failure indication
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

    /// Gracefully shutdown the managed daemon process.
    ///
    /// Only affects daemons spawned by this client. Daemons that were
    /// already running when the client connected are left untouched.
    ///
    /// # Behavior
    ///
    /// 1. Gives daemon 100ms to exit gracefully
    /// 2. Force-kills daemon if still running
    /// 3. Cleans up socket files
    /// 4. Releases process handle
    ///
    /// # Example
    ///
    /// ```rust
    /// use daemon_rpc::prelude::*;
    ///
    /// #[derive(Serialize, Deserialize, Debug, Clone)]
    /// enum MyMethod { Process, GetStatus }
    ///
    /// impl RpcMethod for MyMethod {
    ///     type Response = String;
    /// }
    /// ```
    ///
    /// # Note
    ///
    /// The `Drop` implementation automatically cleans up daemon processes
    /// when the client is dropped, so calling `shutdown()` explicitly is
    /// optional in most cases.
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
