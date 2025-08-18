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
/// version synchronization, and socket-based coordination.
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
    /// - Establishing socket-based coordination
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
    ///     // Client cleans up automatically when dropped
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

        let socket_client = if let Ok(existing_client) = SocketClient::connect(daemon_id).await {
            // Daemon is already running and responsive - use it
            existing_client
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
            _phantom: std::marker::PhantomData,
        })
    }

    async fn spawn_and_wait_for_ready(
        daemon_id: u64,
        daemon_executable: &PathBuf,
        build_timestamp: u64,
    ) -> Result<SocketClient> {
        // Retry daemon spawning to handle race conditions with concurrent test cleanup
        for retry_attempt in 0..3 {
            let result =
                Self::try_spawn_daemon(daemon_id, daemon_executable, build_timestamp).await;

            match result {
                Ok(client) => return Ok(client),
                Err(e) if e.to_string().contains("signal: 15") && retry_attempt < 2 => {
                    // Daemon was killed during startup, likely by concurrent test cleanup
                    // Wait a bit and retry
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Err(anyhow::anyhow!("Failed to spawn daemon after retries"))
    }

    async fn try_spawn_daemon(
        daemon_id: u64,
        daemon_executable: &PathBuf,
        build_timestamp: u64,
    ) -> Result<SocketClient> {
        // Spawn daemon process (detached - it will manage its own lifecycle)
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

            // Check if process crashed during startup
            if let Ok(Some(exit_status)) = child.try_wait() {
                return Err(anyhow::anyhow!(
                    "Daemon process exited during startup with status: {}",
                    exit_status
                ));
            }

            // Try to connect
            if socket_path.exists()
                && let Ok(socket_client) = SocketClient::connect(daemon_id).await
            {
                // Successfully connected - daemon is ready
                return Ok(socket_client);
            }

            if attempts >= MAX_ATTEMPTS {
                // Kill the startup process if it's still running
                let _ = child.kill().await;
                return Err(anyhow::anyhow!("Daemon failed to start within timeout"));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn cleanup_stale_processes(daemon_id: u64) {
        // Best effort cleanup using PID file
        let pid_file = crate::transport::pid_path(daemon_id);
        if pid_file.exists()
            && let Ok(pid_str) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = pid_str.trim().parse::<u32>()
        {
            let _ = tokio::process::Command::new("kill")
                .arg(pid.to_string())
                .output()
                .await;
        }
        // Remove stale PID file
        let _ = std::fs::remove_file(&pid_file);

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
        // Try the request with automatic retry on connection failure (max 2 attempts)
        let mut last_error = None;
        let mut response = None;

        for attempt in 0..2 {
            // Send request over socket
            let send_result = self
                .socket_client
                .send_message(&SocketMessage::Request(RpcRequest {
                    method: method.clone(),
                    client_build_timestamp: self.build_timestamp,
                }))
                .await;

            match send_result {
                Ok(()) => {
                    // Send succeeded, now wait for response
                    match self
                        .socket_client
                        .receive_message::<SocketMessage<M>>()
                        .await
                    {
                        Ok(Some(message)) => match message {
                            SocketMessage::Response(resp) => {
                                response = Some(resp);
                                break;
                            }
                            _ => return Err(anyhow::anyhow!("Unexpected message type")),
                        },
                        Ok(None) => {
                            last_error = Some("Socket connection closed".to_string());
                        }
                        Err(e) => {
                            last_error = Some(format!("Failed to receive response: {}", e));
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(format!("Failed to send request: {}", e));
                }
            }

            // If we get here, something failed
            if attempt == 0 {
                // First failure, try to reconnect
                if let Err(e) = self.ensure_connection().await {
                    return Err(anyhow::anyhow!("Failed to reconnect: {}", e));
                }
            }
            // For second failure, we'll exit the loop and return an error
        }

        let response = response.ok_or_else(|| {
            anyhow::anyhow!(
                "Request failed after retry: {}",
                last_error.unwrap_or_else(|| "Unknown error".to_string())
            )
        })?;

        // Handle version mismatch by restarting daemon
        if let RpcResponse::VersionMismatch {
            daemon_build_timestamp,
        } = &response
        {
            println!(
                "Version mismatch detected: client={}, daemon={}. Restarting daemon...",
                self.build_timestamp, daemon_build_timestamp
            );

            // Restart daemon to fix version mismatch
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
        // Clean up old socket file to signal daemon should exit
        let socket_path = socket_path(self.daemon_id);
        let _ = std::fs::remove_file(&socket_path);

        // Kill any zombie processes (best effort)
        Self::cleanup_stale_processes(self.daemon_id).await;

        // Give the old daemon time to exit gracefully
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Spawn new daemon process
        let socket_client = Self::spawn_and_wait_for_ready(
            self.daemon_id,
            &self.daemon_executable,
            self.build_timestamp,
        )
        .await?;

        // Update socket client
        self.socket_client = socket_client;

        Ok(())
    }

    /// Attempt to reconnect to the daemon or restart it if necessary
    async fn ensure_connection(&mut self) -> Result<()> {
        // First try to reconnect to existing daemon
        if let Ok(socket_client) = SocketClient::connect(self.daemon_id).await {
            self.socket_client = socket_client;
            return Ok(());
        }

        // Connection failed, restart daemon
        self.restart_daemon().await
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
