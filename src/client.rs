use crate::error_context::{ErrorContextBuffer, get_or_init_global_error_context};
use crate::process::{TerminateResult, kill_process, process_exists, terminate_process};
use crate::terminal::TerminalInfo;
use crate::transport::{SocketClient, SocketMessage, daemon_socket_exists, socket_path};
use anyhow::{Result, bail};
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, time::sleep};

/// Client for communicating with daemon processes via Unix sockets.
///
/// Provides zero-configuration daemon management with automatic spawning,
/// version synchronization via binary mtime comparison, and stdin/stdout streaming.
///
/// # Version Management
///
/// The client automatically detects when the binary has been rebuilt by comparing
/// modification times (mtime). If the client binary is newer than the running daemon,
/// the daemon is automatically restarted to ensure version consistency.
///
/// # Example
///
/// ```rust
/// use daemon_cli::prelude::*;
///
/// // Actual usage pattern (requires daemon binary):
///  # tokio_test::block_on(async {
///      let Ok(mut client) = DaemonClient::connect("/path/to/project").await else {
///         // handle error...
///         return;
///      };
///
///      // Execute a command - output is streamed to stdout, returns exit code
///      let exit_code = client.execute_command("process file.txt".to_string()).await.ok();
///  # });
/// ```
pub struct DaemonClient {
    socket_client: SocketClient,
    /// Daemon name (e.g., CLI tool name)
    pub daemon_name: String,
    /// Project root directory path (used as unique identifier/scope)
    pub root_path: String,
    /// Path to the daemon executable for spawning
    pub daemon_executable: PathBuf,
    /// Binary modification time (mtime) for version compatibility checking
    pub build_timestamp: u64,
    /// Error context buffer for client-side logging
    error_context: ErrorContextBuffer,
    /// Enable automatic daemon restart on fatal connection errors (default: false)
    auto_restart_on_error: bool,
}

impl DaemonClient {
    /// Connect to daemon, spawning it if needed with automatic version sync.
    ///
    /// Automatically detects the daemon name from the binary filename, the daemon
    /// executable path (current binary), and the binary's modification time for
    /// version checking. Handles daemon detection, spawning, readiness waiting,
    /// and version handshake. Restarts daemon on version mismatch.
    pub async fn connect(root_path: &str) -> Result<Self> {
        let daemon_name = crate::auto_detect_daemon_name();
        let daemon_executable = std::env::current_exe()
            .map_err(|e| anyhow::anyhow!("Failed to get current executable path: {}", e))?;
        let build_timestamp = crate::get_build_timestamp();
        Self::connect_with_name_and_timestamp(
            &daemon_name,
            root_path,
            daemon_executable,
            build_timestamp,
        )
        .await
    }

    /// Connect to daemon with explicit name and timestamp (primarily for testing).
    ///
    /// Most users should use [`connect()`](Self::connect) which auto-detects the daemon name
    /// and binary modification time. This method allows full control for test isolation and
    /// version mismatch scenarios.
    ///
    /// Handles daemon detection, spawning, readiness waiting, and version
    /// handshake. Restarts daemon on version mismatch.
    pub async fn connect_with_name_and_timestamp(
        daemon_name: &str,
        root_path: &str,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        // Get or initialize the global error context buffer (shared across all clients)
        let error_context = get_or_init_global_error_context();

        tracing::debug!(daemon_name, root_path, "Connecting to daemon");

        // Try to connect to existing daemon first
        let socket_path = socket_path(daemon_name, root_path);

        let mut socket_client = if let Ok(existing_client) =
            SocketClient::connect(daemon_name, root_path).await
        {
            // Daemon is already running and responsive - use it
            tracing::debug!("Connected to existing daemon");
            existing_client
        } else {
            // Daemon not running or not responsive - spawn our own
            tracing::debug!("No existing daemon found, spawning new daemon");

            if daemon_socket_exists(daemon_name, root_path) {
                // Clean up stale socket file
                tracing::debug!("Cleaning up stale socket file");
                let _ = fs::remove_file(&socket_path);
            }

            // Kill any zombie processes (best effort)
            Self::cleanup_stale_processes(daemon_name, root_path).await;

            // Spawn new daemon
            match Self::spawn_and_wait_for_ready(daemon_name, root_path, &daemon_executable).await {
                Ok(client) => client,
                Err(e) => {
                    error_context.dump_to_stderr();
                    return Err(e);
                }
            }
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
            tracing::info!(
                client_version = build_timestamp,
                daemon_version = daemon_timestamp,
                "Version mismatch detected, restarting daemon"
            );

            // Clean up and restart
            let _ = fs::remove_file(&socket_path);
            Self::cleanup_stale_processes(daemon_name, root_path).await;

            // Spawn new daemon with correct version
            socket_client =
                match Self::spawn_and_wait_for_ready(daemon_name, root_path, &daemon_executable)
                    .await
                {
                    Ok(client) => client,
                    Err(e) => {
                        error_context.dump_to_stderr();
                        return Err(e);
                    }
                };

            // Retry handshake
            socket_client
                .send_message(&SocketMessage::VersionCheck { build_timestamp })
                .await?;
            match socket_client.receive_message().await? {
                Some(SocketMessage::VersionCheck {
                    build_timestamp: daemon_ts,
                }) if daemon_ts == build_timestamp => {
                    tracing::debug!("Version handshake successful after restart");
                    eprintln!("Daemon restarted (binary was updated)");
                }
                _ => {
                    error_context.dump_to_stderr();
                    bail!("Version handshake failed after restart");
                }
            }
        } else {
            tracing::debug!("Version handshake successful");
        }

        tracing::debug!("Successfully connected to daemon");

        Ok(Self {
            socket_client,
            daemon_name: daemon_name.to_string(),
            root_path: root_path.to_string(),
            daemon_executable,
            build_timestamp,
            error_context,
            auto_restart_on_error: false,
        })
    }

    async fn spawn_and_wait_for_ready(
        daemon_name: &str,
        root_path: &str,
        daemon_executable: &PathBuf,
    ) -> Result<SocketClient> {
        tracing::debug!(daemon_exe = ?daemon_executable, "Spawning daemon");

        // Retry daemon spawning to handle race conditions with concurrent test cleanup
        for retry_attempt in 0..3 {
            let result = Self::try_spawn_daemon(daemon_name, root_path, daemon_executable).await;

            match result {
                Ok(client) => {
                    tracing::debug!("Daemon spawned successfully");
                    return Ok(client);
                }
                Err(e) if e.to_string().contains("signal: 15") && retry_attempt < 2 => {
                    // Daemon was killed during startup, likely by concurrent test cleanup
                    // Wait a bit and retry
                    tracing::debug!(
                        retry = retry_attempt + 1,
                        "Daemon killed during startup, retrying"
                    );
                    sleep(Duration::from_millis(300)).await;
                    continue;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to spawn daemon");
                    return Err(e);
                }
            }
        }

        bail!("Failed to spawn daemon after retries")
    }

    async fn try_spawn_daemon(
        daemon_name: &str,
        root_path: &str,
        daemon_executable: &PathBuf,
    ) -> Result<SocketClient> {
        // Spawn daemon process (detached - it will manage its own lifecycle)
        // The daemon will auto-detect its binary mtime for version checking
        tracing::debug!("Starting daemon process");
        let mut child = Command::new(daemon_executable)
            .arg("daemon")
            .arg("--daemon-name")
            .arg(daemon_name)
            .arg("--root-path")
            .arg(root_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn daemon process: {}", e))?;

        // Wait for daemon to become ready
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 50; // 5 seconds total

        tracing::debug!("Waiting for daemon to become ready");

        loop {
            attempts += 1;

            // Check if process crashed during startup
            if let Ok(Some(exit_status)) = child.try_wait() {
                tracing::error!(exit_status = %exit_status, "Daemon process exited during startup");
                bail!(
                    "Daemon process exited during startup with status: {}",
                    exit_status
                );
            }

            // Try to connect
            if daemon_socket_exists(daemon_name, root_path)
                && let Ok(socket_client) = SocketClient::connect(daemon_name, root_path).await
            {
                // Successfully connected - daemon is ready
                tracing::debug!("Daemon ready and accepting connections");
                return Ok(socket_client);
            }

            if attempts >= MAX_ATTEMPTS {
                // Kill the startup process if it's still running
                let _ = child.kill().await;
                tracing::error!("Daemon failed to start within timeout");
                bail!("Daemon failed to start within timeout");
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn cleanup_stale_processes(daemon_name: &str, root_path: &str) {
        // Best effort cleanup using PID file
        let pid_file = crate::transport::pid_path(daemon_name, root_path);
        if pid_file.exists()
            && let Ok(pid_str) = fs::read_to_string(&pid_file)
            && let Ok(pid) = pid_str.trim().parse::<u32>()
        {
            tracing::debug!(pid, "Cleaning up stale daemon process");
            kill_process(pid).await;
        }
        // Remove stale PID file
        let _ = fs::remove_file(&pid_file);

        sleep(Duration::from_millis(100)).await;
    }

    /// Force-stop the daemon process.
    ///
    /// This method will:
    /// 1. Read the daemon's PID from the PID file
    /// 2. Attempt graceful shutdown (Unix: SIGTERM, Windows: immediate termination)
    /// 3. Wait up to 1 second for the process to exit (Unix only)
    /// 4. Force terminate if still running (Unix: SIGKILL)
    /// 5. Clean up PID and socket files
    ///
    /// # Platform Behavior
    ///
    /// - **Unix**: Sends SIGTERM for graceful shutdown, waits up to 1 second,
    ///   then sends SIGKILL if still running.
    /// - **Windows**: Immediately terminates the process. Windows has no
    ///   SIGTERM equivalent for console applications.
    ///
    /// Returns an error if the daemon is not running or cannot be stopped.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use daemon_cli::prelude::*;
    ///
    /// # tokio_test::block_on(async {
    /// let client = DaemonClient::connect("/path/to/project").await?;
    /// client.force_stop().await?;
    /// # Ok::<(), anyhow::Error>(())
    /// # });
    /// ```
    pub async fn force_stop(&self) -> Result<()> {
        let pid_file = crate::transport::pid_path(&self.daemon_name, &self.root_path);
        let sock_path = socket_path(&self.daemon_name, &self.root_path);

        // Read PID file
        if !pid_file.exists() {
            // Best-effort cleanup of potentially stale socket
            let _ = fs::remove_file(&sock_path);
            bail!("Daemon is not running (no PID file found)");
        }

        let pid_str = fs::read_to_string(&pid_file)
            .map_err(|e| anyhow::anyhow!("Failed to read PID file: {}", e))?;
        let pid: u32 = pid_str
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid PID in file: {}", e))?;

        tracing::info!(pid, "Force-stopping daemon");

        // Check if process exists
        match process_exists(pid) {
            Ok(true) => {
                // Process exists, continue
            }
            Ok(false) => {
                // Process doesn't exist, clean up files
                tracing::warn!(pid, "Process not running, cleaning up files");
                let _ = fs::remove_file(&pid_file);
                let _ = fs::remove_file(&sock_path);
                bail!("Daemon process (PID {}) is not running", pid);
            }
            Err(e) => {
                bail!("Error checking daemon process (PID {}): {}", pid, e);
            }
        }

        // Terminate with 1 second graceful timeout (Unix only; Windows is immediate)
        match terminate_process(pid, 1000).await {
            TerminateResult::Terminated => {
                tracing::info!(pid, "Daemon stopped");
            }
            TerminateResult::AlreadyDead => {
                tracing::info!(pid, "Daemon was already stopped");
            }
            TerminateResult::PermissionDenied => {
                bail!(
                    "Permission denied: cannot stop daemon process (PID {}). \
                     Process appears to be running but owned by another user.",
                    pid
                );
            }
            TerminateResult::Error(e) => {
                bail!("Failed to stop daemon (PID {}): {}", pid, e);
            }
        }

        // Clean up files
        let _ = fs::remove_file(&pid_file);
        let _ = fs::remove_file(&sock_path);

        Ok(())
    }

    /// Restart the daemon by force-stopping it and reconnecting.
    ///
    /// This is useful when the daemon has crashed or become unresponsive.
    /// It will:
    /// 1. Force-stop the existing daemon (if running)
    /// 2. Reconnect to a fresh daemon instance
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use daemon_cli::prelude::*;
    ///
    /// # tokio_test::block_on(async {
    /// let mut client = DaemonClient::connect("/path/to/project").await?;
    ///
    /// // If daemon crashes or hangs:
    /// client.restart().await?;
    /// # Ok::<(), anyhow::Error>(())
    /// # });
    /// ```
    pub async fn restart(&mut self) -> Result<()> {
        tracing::info!("Restarting daemon");

        // Force stop existing daemon (ignore errors if already dead)
        let _ = self.force_stop().await;

        // Reconnect to fresh daemon
        let new_client = Self::connect_with_name_and_timestamp(
            &self.daemon_name,
            &self.root_path,
            self.daemon_executable.clone(),
            self.build_timestamp,
        )
        .await?;

        // Replace self with new client, preserving auto_restart setting
        let auto_restart = self.auto_restart_on_error;
        *self = new_client;
        self.auto_restart_on_error = auto_restart;

        Ok(())
    }

    /// Enable or disable automatic daemon restart on fatal connection errors.
    ///
    /// When enabled, if `execute_command()` encounters a fatal connection error
    /// (daemon crash, broken pipe, etc.), it will automatically restart the daemon
    /// and retry the command once.
    ///
    /// Default: false (manual recovery required)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use daemon_cli::prelude::*;
    ///
    /// # tokio_test::block_on(async {
    /// let mut client = DaemonClient::connect("/path/to/project")
    ///     .await?
    ///     .with_auto_restart(true);
    ///
    /// // If daemon crashes, command will automatically retry after restart
    /// client.execute_command("process file.txt".to_string()).await?;
    /// # Ok::<(), anyhow::Error>(())
    /// # });
    /// ```
    pub fn with_auto_restart(mut self, enabled: bool) -> Self {
        self.auto_restart_on_error = enabled;
        self
    }

    /// Check if an error indicates a fatal connection issue (daemon crash/hang).
    ///
    /// Returns true for errors that suggest the daemon has crashed or become
    /// unresponsive (broken pipe, connection reset, etc.). Returns false for
    /// normal errors or transient issues.
    fn is_fatal_connection_error(error: &anyhow::Error) -> bool {
        let error_str = error.to_string().to_lowercase();

        // Check for clear indicators of daemon crash/hang
        error_str.contains("broken pipe")
            || error_str.contains("connection reset")
            || error_str.contains("connection closed unexpectedly")
            || error_str.contains("connection refused")
            || error_str.contains("not connected")
    }

    /// Execute a command on the daemon and stream output to stdout.
    ///
    /// Streams output chunks as they arrive. Errors written to stderr.
    /// Ctrl+C cancels via connection close.
    ///
    /// If `auto_restart_on_error` is enabled (via `with_auto_restart(true)`),
    /// the daemon will be automatically restarted and the command retried once
    /// on fatal connection errors.
    ///
    /// Returns the command's exit code (0 for success, non-zero for errors).
    pub async fn execute_command(&mut self, command: String) -> Result<i32> {
        // Try to execute command
        let result = self.execute_command_internal(command.clone()).await;

        // Check if we should auto-restart on error
        if let Err(ref error) = result
            && self.auto_restart_on_error
            && Self::is_fatal_connection_error(error)
        {
            tracing::warn!(
                error = %error,
                "Fatal connection error detected, restarting daemon and retrying"
            );

            // Restart daemon
            if let Err(restart_err) = self.restart().await {
                tracing::error!(error = %restart_err, "Failed to restart daemon");
                return Err(anyhow::anyhow!(
                    "Daemon crashed and restart failed: {}",
                    restart_err
                ));
            }

            // Retry command once
            tracing::info!("Retrying command after daemon restart");
            return self.execute_command_internal(command).await;
        }

        result
    }

    /// Internal implementation of command execution (without auto-restart logic).
    async fn execute_command_internal(&mut self, command: String) -> Result<i32> {
        tracing::debug!(command = %command, "Executing command");

        // Detect terminal information from the client environment
        let terminal_info = TerminalInfo::detect().await;
        tracing::debug!(
            width = ?terminal_info.width,
            height = ?terminal_info.height,
            is_tty = terminal_info.is_tty,
            color_support = ?terminal_info.color_support,
            "Detected terminal info"
        );

        // Send command with terminal info
        self.socket_client
            .send_message(&SocketMessage::Command {
                command,
                terminal_info,
            })
            .await
            .inspect_err(|_| {
                self.error_context.dump_to_stderr();
            })?;

        // Stream output chunks to stdout
        let mut stdout = tokio::io::stdout();

        loop {
            match self.socket_client.receive_message::<SocketMessage>().await {
                Ok(Some(SocketMessage::OutputChunk(chunk))) => {
                    // Write chunk to stdout
                    stdout.write_all(&chunk).await?;
                    stdout.flush().await?;
                }
                Ok(Some(SocketMessage::CommandComplete { exit_code })) => {
                    // Command completed with exit code
                    tracing::debug!(exit_code = exit_code, "Command completed");
                    return Ok(exit_code);
                }
                Ok(Some(SocketMessage::CommandError(error))) => {
                    // Write error to stderr
                    eprintln!("Error: {}", error);
                    return Err(anyhow::anyhow!("Command failed: {}", error));
                }
                Ok(None) => {
                    // Connection closed unexpectedly
                    tracing::error!("Connection closed unexpectedly");
                    self.error_context.dump_to_stderr();
                    return Err(anyhow::anyhow!("Connection closed unexpectedly"));
                }
                Ok(_) => {
                    // Unexpected message type
                    tracing::error!("Unexpected message from daemon");
                    self.error_context.dump_to_stderr();
                    bail!("Unexpected message from daemon");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to receive message from daemon");
                    self.error_context.dump_to_stderr();
                    return Err(e);
                }
            }
        }
    }
}
