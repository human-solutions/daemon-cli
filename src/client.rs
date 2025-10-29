use crate::transport::{SocketClient, SocketMessage, socket_path};
use anyhow::{Result, bail};
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, time::sleep};

/// Client for communicating with daemon processes via Unix sockets.
///
/// Provides zero-configuration daemon management with automatic spawning,
/// version synchronization, and stdin/stdout streaming.
///
/// # Example
///
/// ```rust
/// use daemon_rpc::prelude::*;
/// use std::path::PathBuf;
///
/// let daemon_exe = PathBuf::from("./target/debug/examples/cli");
/// let build_timestamp = 1234567890u64;
///
/// // Actual usage pattern (requires daemon binary):
///  # tokio_test::block_on(async {
///      let Ok(mut client) = DaemonClient::connect(1000, daemon_exe, build_timestamp).await else {
///         // handle error...
///         return;
///      };
///
///      // Execute a command - output is streamed to stdout
///      client.execute_command("process file.txt".to_string()).await.ok();
///  # });
/// ```
pub struct DaemonClient {
    socket_client: SocketClient,
    /// Unique identifier for this daemon instance
    pub daemon_id: u64,
    /// Path to the daemon executable for spawning
    pub daemon_executable: PathBuf,
    /// Build timestamp for version compatibility checking
    pub build_timestamp: u64,
}

impl DaemonClient {
    /// Connect to daemon, spawning it if needed with automatic version sync.
    ///
    /// Handles daemon detection, spawning, readiness waiting, and version
    /// handshake. Restarts daemon on version mismatch.
    pub async fn connect(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        // Try to connect to existing daemon first
        let socket_path = socket_path(daemon_id);

        let mut socket_client = if let Ok(existing_client) = SocketClient::connect(daemon_id).await
        {
            // Daemon is already running and responsive - use it
            existing_client
        } else {
            // Daemon not running or not responsive - spawn our own
            if socket_path.exists() {
                // Clean up stale socket file
                let _ = fs::remove_file(&socket_path);
            }

            // Kill any zombie processes (best effort)
            Self::cleanup_stale_processes(daemon_id).await;

            // Spawn new daemon
            Self::spawn_and_wait_for_ready(daemon_id, &daemon_executable, build_timestamp).await?
        };

        // Perform version handshake
        socket_client
            .send_message(&SocketMessage::VersionCheck { build_timestamp })
            .await?;

        // Receive daemon's version
        let daemon_timestamp = match socket_client.receive_message().await? {
            Some(SocketMessage::VersionCheck {
                build_timestamp: daemon_ts,
            }) => daemon_ts,
            _ => bail!("Invalid version handshake response"),
        };

        // If versions don't match, restart daemon
        if daemon_timestamp != build_timestamp {
            println!(
                "Version mismatch detected: client={}, daemon={}. Restarting daemon...",
                build_timestamp, daemon_timestamp
            );

            // Clean up and restart
            let _ = fs::remove_file(&socket_path);
            Self::cleanup_stale_processes(daemon_id).await;

            // Spawn new daemon with correct version
            socket_client =
                Self::spawn_and_wait_for_ready(daemon_id, &daemon_executable, build_timestamp)
                    .await?;

            // Retry handshake
            socket_client
                .send_message(&SocketMessage::VersionCheck { build_timestamp })
                .await?;
            match socket_client.receive_message().await? {
                Some(SocketMessage::VersionCheck {
                    build_timestamp: daemon_ts,
                }) if daemon_ts == build_timestamp => {}
                _ => bail!("Version handshake failed after restart"),
            }
        }

        Ok(Self {
            socket_client,
            daemon_id,
            daemon_executable,
            build_timestamp,
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
                    sleep(Duration::from_millis(300)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        bail!("Failed to spawn daemon after retries")
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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
                bail!(
                    "Daemon process exited during startup with status: {}",
                    exit_status
                );
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
                bail!("Daemon failed to start within timeout");
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn cleanup_stale_processes(daemon_id: u64) {
        // Best effort cleanup using PID file
        let pid_file = crate::transport::pid_path(daemon_id);
        if pid_file.exists()
            && let Ok(pid_str) = fs::read_to_string(&pid_file)
            && let Ok(pid) = pid_str.trim().parse::<u32>()
        {
            let _ = Command::new("kill").arg(pid.to_string()).output().await;
        }
        // Remove stale PID file
        let _ = fs::remove_file(&pid_file);

        sleep(Duration::from_millis(100)).await;
    }

    /// Execute a command on the daemon and stream output to stdout.
    ///
    /// Streams output chunks as they arrive. Errors written to stderr.
    /// Ctrl+C cancels via connection close.
    pub async fn execute_command(&mut self, command: String) -> Result<()> {
        // Send command
        self.socket_client
            .send_message(&SocketMessage::Command(command))
            .await?;

        // Stream output chunks to stdout
        let mut stdout = tokio::io::stdout();

        loop {
            match self
                .socket_client
                .receive_message::<SocketMessage>()
                .await?
            {
                Some(SocketMessage::OutputChunk(chunk)) => {
                    // Write chunk to stdout
                    stdout.write_all(&chunk).await?;
                    stdout.flush().await?;
                }
                Some(SocketMessage::CommandError(error)) => {
                    // Write error to stderr
                    eprintln!("Error: {}", error);
                    return Err(anyhow::anyhow!("Command failed: {}", error));
                }
                None => {
                    // Connection closed - command completed successfully
                    return Ok(());
                }
                _ => {
                    // Unexpected message type
                    bail!("Unexpected message from daemon");
                }
            }
        }
    }
}

// #[cfg(test)]
// #[path = "client_tests.rs"]
// mod tests;
