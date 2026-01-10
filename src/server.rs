use crate::transport::{SocketMessage, SocketServer};
use crate::*;
use anyhow::Result;
use std::{
    fs,
    io::ErrorKind,
    marker::PhantomData,
    process,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    select, spawn,
    sync::{Semaphore, oneshot},
    time::sleep,
};
use tracing::Instrument;

/// Global client connection counter for logging context
static CLIENT_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Daemon server that processes commands from CLI clients.
///
/// Handles multiple concurrent clients with streaming output,
/// cancellation support, and mtime-based version checking.
///
/// # Version Management
///
/// The server stores the binary's modification time (mtime) and compares it
/// with connecting clients. If a client has a newer mtime, the client will
/// automatically restart the daemon to ensure version consistency.
///
/// # Example
///
/// ```rust
/// use daemon_cli::prelude::*;
/// use tokio::io::{AsyncWrite, AsyncWriteExt};
///
/// #[derive(Clone)]
/// struct MyDaemon;
///
/// #[async_trait]
/// impl CommandHandler for MyDaemon {
///     async fn handle(
///         &self,
///         command: &str,
///         _ctx: CommandContext,
///         mut output: impl AsyncWrite + Send + Unpin,
///         _cancel_token: CancellationToken,
///     ) -> Result<i32> {
///         output.write_all(b"Processed: ").await?;
///         output.write_all(command.as_bytes()).await?;
///         Ok(0)
///     }
/// }
///
/// // Demonstrate server creation - automatically detects daemon name and binary mtime
/// let daemon = MyDaemon;
/// let (server, _handle) = DaemonServer::new("/path/to/project", daemon, StartupReason::FirstStart);
/// // Use handle.shutdown() to stop the server, or drop it to run indefinitely
/// ```
pub struct DaemonServer<H, P = ()> {
    /// Daemon name (e.g., CLI tool name)
    pub daemon_name: String,
    /// Project root path (used as unique identifier/scope)
    pub root_path: String,
    /// Binary modification time (mtime) for version compatibility checking
    pub build_timestamp: u64,
    /// Reason why this daemon instance was started
    pub startup_reason: StartupReason,
    handler: H,
    shutdown_rx: oneshot::Receiver<()>,
    connection_semaphore: Arc<Semaphore>,
    _phantom: PhantomData<P>,
}

/// Handle for controlling a running daemon server.
///
/// Call `shutdown()` to gracefully stop the server, or drop the handle
/// to let the server run indefinitely.
pub struct DaemonHandle {
    shutdown_tx: oneshot::Sender<()>,
}

impl DaemonHandle {
    /// Signal the daemon to shut down gracefully.
    ///
    /// Sends a shutdown signal to the server, causing it to stop accepting
    /// new connections and exit cleanly.
    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

impl<H, P> DaemonServer<H, P>
where
    H: CommandHandler<P> + Clone + 'static,
    P: PayloadCollector,
{
    /// Create a new daemon server instance with default connection limit (100).
    ///
    /// Automatically detects the daemon name from the binary filename and the
    /// binary's modification time for version checking. Returns the server and
    /// a handle that can be used to shut it down gracefully.
    ///
    /// # Parameters
    ///
    /// * `root_path` - Project root directory path used as unique identifier/scope for this daemon instance
    /// * `handler` - Your command handler implementation
    /// * `startup_reason` - Why the daemon is starting (passed from client via `--startup-reason` CLI arg)
    ///
    /// # Returns
    ///
    /// A tuple of (DaemonServer, DaemonHandle). Call `shutdown()` on the handle
    /// to gracefully stop the server, or drop it to let the server run indefinitely.
    ///
    pub fn new(root_path: &str, handler: H, startup_reason: StartupReason) -> (Self, DaemonHandle) {
        let daemon_name = crate::auto_detect_daemon_name();
        let build_timestamp = crate::get_build_timestamp();
        Self::new_with_name_and_timestamp(
            &daemon_name,
            root_path,
            build_timestamp,
            handler,
            startup_reason,
            100,
        )
    }

    /// Create a new daemon server instance with explicit name, timestamp, and connection limit (primarily for testing).
    ///
    /// Most users should use [`new()`](Self::new) which auto-detects the daemon name and
    /// binary modification time. This method allows full control for test isolation and
    /// version mismatch scenarios.
    ///
    /// # Parameters
    ///
    /// * `daemon_name` - Name of the daemon (e.g., CLI tool name)
    /// * `root_path` - Project root directory path used as unique identifier/scope for this daemon instance
    /// * `build_timestamp` - Binary mtime (seconds since Unix epoch) for version checking
    /// * `handler` - Your command handler implementation
    /// * `startup_reason` - Why the daemon is starting (passed from client via `--startup-reason` CLI arg)
    /// * `max_connections` - Maximum number of concurrent client connections
    ///
    /// # Returns
    ///
    /// A tuple of (DaemonServer, DaemonHandle). Call `shutdown()` on the handle
    /// to gracefully stop the server, or drop it to let the server run indefinitely.
    ///
    pub fn new_with_name_and_timestamp(
        daemon_name: &str,
        root_path: &str,
        build_timestamp: u64,
        handler: H,
        startup_reason: StartupReason,
        max_connections: usize,
    ) -> (Self, DaemonHandle) {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Create semaphore for connection limiting
        let connection_semaphore = Arc::new(Semaphore::new(max_connections));

        let server = Self {
            daemon_name: daemon_name.to_string(),
            root_path: root_path.to_string(),
            build_timestamp,
            startup_reason,
            handler,
            shutdown_rx,
            connection_semaphore,
            _phantom: PhantomData,
        };
        let handle = DaemonHandle { shutdown_tx };
        (server, handle)
    }

    /// Start the daemon server and listen for client connections.
    ///
    /// Creates a Unix socket and processes incoming commands with streaming output.
    /// This method blocks until the daemon is shut down.
    ///
    /// # Returns
    ///
    /// Returns when the server shuts down, or an error if startup fails
    ///
    /// # Behavior
    ///
    /// - Creates Unix socket at `/tmp/{short_id}-{daemon_name}.sock`
    /// - Sets socket permissions to 0600 (owner read/write only)
    /// - Accepts multiple concurrent client connections
    /// - Each client handled in separate tokio task
    /// - Performs version handshake on each connection
    /// - Streams output as it's generated
    /// - Handles task cancellation via connection close detection
    /// - Shuts down gracefully if shutdown signal is received
    ///
    /// # Concurrency
    ///
    /// The server spawns a separate task for each client connection, allowing
    /// multiple clients to execute commands concurrently. Handlers must be
    /// thread-safe if they access shared mutable state (use [`Arc<Mutex<T>>`](std::sync::Arc) or
    /// similar synchronization primitives).
    pub async fn run(mut self) -> Result<()> {
        let mut socket_server = SocketServer::new(&self.daemon_name, &self.root_path).await?;

        // Write PID file for precise process management
        let pid = process::id();
        let pid_file = crate::transport::pid_path(&self.daemon_name, &self.root_path);
        if let Err(e) = fs::write(&pid_file, pid.to_string()) {
            tracing::warn!(pid_file = ?pid_file, error = %e, "Failed to write PID file");
        }

        // Ensure PID file cleanup on exit
        let cleanup_pid_file = pid_file.clone();
        let _cleanup_guard = scopeguard::guard((), move |_| {
            let _ = fs::remove_file(&cleanup_pid_file);
        });

        tracing::info!(
            daemon_name = %self.daemon_name,
            root_path = %self.root_path,
            socket_path = ?socket_server.socket_path(),
            build_timestamp = self.build_timestamp,
            "Daemon started and listening"
        );

        // Notify handler of startup reason and PID
        self.handler.on_startup(self.startup_reason, pid);

        loop {
            // Select between accepting connection and shutdown signal
            let accept_result = select! {
                result = socket_server.accept() => Some(result),
                _ = &mut self.shutdown_rx => {
                    tracing::info!(daemon_name = %self.daemon_name, "Daemon received shutdown signal");
                    break;
                }
            };

            match accept_result {
                Some(Ok(mut connection)) => {
                    // Try to acquire connection permit immediately (non-blocking)
                    // This prevents holding file descriptors while waiting for capacity
                    let permit = match self.connection_semaphore.clone().try_acquire_owned() {
                        Ok(permit) => permit,
                        Err(tokio::sync::TryAcquireError::NoPermits) => {
                            // No permits available - drop connection immediately
                            tracing::warn!("Connection limit reached, rejecting new connection");
                            // Connection is dropped here, freeing the file descriptor
                            // Client will receive connection closed/reset and can retry
                            continue;
                        }
                        Err(tokio::sync::TryAcquireError::Closed) => {
                            tracing::error!("Semaphore closed, shutting down");
                            break;
                        }
                    };

                    // Clone values needed for the task after we have a permit
                    let handler = self.handler.clone();
                    let build_timestamp = self.build_timestamp;
                    let client_id = CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed);

                    // Handle this client connection
                    // Instrument with client span to properly handle async .await points
                    spawn(
                        async move {
                            // Permit is held for the lifetime of this task and released when dropped
                            let _permit = permit;

                            tracing::debug!("Connection accepted");

                        // Version handshake
                        if let Ok(Some(SocketMessage::<P>::VersionCheck {
                            build_timestamp: client_timestamp,
                        })) = connection.receive_message::<SocketMessage<P>>().await
                        {
                            // Send our build timestamp
                            if connection
                                .send_message(&SocketMessage::<P>::VersionCheck { build_timestamp })
                                .await
                                .is_err()
                            {
                                tracing::debug!("Failed to send version check response");
                                return;
                            }

                            // If versions don't match, client will restart us - just wait for disconnect
                            if client_timestamp != build_timestamp {
                                tracing::info!(
                                    client_version = client_timestamp,
                                    server_version = build_timestamp,
                                    "Version mismatch detected, client will restart daemon"
                                );
                                return;
                            }

                            tracing::debug!("Version handshake successful");
                        } else {
                            // No version check received
                            tracing::warn!("No version check received from client");
                            return;
                        }

                        // Receive command
                        let (command, context) = match connection.receive_message::<SocketMessage<P>>().await {
                            Ok(Some(SocketMessage::Command { command, context })) => (command, context),
                            _ => {
                                tracing::warn!("No command received from client");
                                return;
                            }
                        };

                        tracing::debug!(
                            terminal_width = ?context.terminal_info.width,
                            terminal_height = ?context.terminal_info.height,
                            is_tty = context.terminal_info.is_tty,
                            color_support = ?context.terminal_info.color_support,
                            "Received command with context"
                        );

                        // Create a pipe for streaming output
                        let (output_writer, mut output_reader) = tokio::io::duplex(8192);

                        // Create cancellation token
                        let cancel_token = tokio_util::sync::CancellationToken::new();
                        let cancel_token_clone = cancel_token.clone();

                        // Spawn handler task (will not inherit span by default)
                        let mut handler_task = Some(spawn(async move {
                            handler
                                .handle(&command, context, output_writer, cancel_token_clone)
                                .await
                        }));

                        // Stream output chunks to client
                        let mut buffer = vec![0u8; 4096];
                        let mut handler_exit_code: Option<i32> = None;
                        let mut handler_error: Option<String> = None;
                        let stream_result = loop {
                            select! {
                                // Read from handler output
                                read_result = output_reader.read(&mut buffer) => {
                                    match read_result {
                                        Ok(0) => {
                                            // CRITICAL FIX: If handler task hasn't been polled yet, check it now
                                            // This ensures we capture the result before sending completion message
                                            if let Some(task) = handler_task.take() {
                                                match task.await {
                                                    Ok(Ok(exit_code)) => {
                                                        handler_exit_code = Some(exit_code);
                                                    }
                                                    Ok(Err(e)) => {
                                                        handler_error = Some(e.to_string());
                                                    }
                                                    Err(e) => {
                                                        handler_error = Some(format!("Task panicked: {}", e));
                                                    }
                                                }
                                            }

                                            // EOF - handler closed output
                                            // Send completion message (error or success with exit code)
                                            let result = if let Some(ref error) = handler_error {
                                                tracing::error!(error = %error, "Handler failed");
                                                let _ = connection.send_message(&SocketMessage::<P>::CommandError(error.clone())).await;
                                                let _ = connection.flush().await;
                                                Err(anyhow::anyhow!("{}", error))
                                            } else {
                                                let exit_code = handler_exit_code.unwrap_or(0);
                                                tracing::debug!(exit_code = exit_code, "Handler completed");
                                                let _ = connection.send_message(&SocketMessage::<P>::CommandComplete { exit_code }).await;
                                                let _ = connection.flush().await;
                                                Ok(())
                                            };

                                            // Wait for client to close connection with timeout
                                            // This ensures the message is received before connection closes
                                            let _ = tokio::time::timeout(
                                                Duration::from_secs(5),
                                                connection.receive_message::<SocketMessage<P>>()
                                            ).await;

                                            break result;
                                        }
                                        Ok(n) => {
                                            // Send chunk to client
                                            let chunk = buffer[..n].to_vec();
                                            if connection.send_message(&SocketMessage::<P>::OutputChunk(chunk)).await.is_err() {
                                                // Connection closed - cancel handler
                                                tracing::warn!("Connection closed by client");
                                                cancel_token.cancel();
                                                break Err(anyhow::anyhow!("Connection closed"));
                                            }
                                        }
                                        Err(e) => {
                                            break Err(anyhow::anyhow!("Read error: {}", e));
                                        }
                                    }
                                }

                                // Handler task completed (only poll if task still exists)
                                task_result = async { handler_task.as_mut().unwrap().await }, if handler_task.is_some() => {
                                    // Take the task so we don't poll it again
                                    handler_task.take();

                                    match task_result {
                                        Ok(Ok(exit_code)) => {
                                            // Handler succeeded - save exit code and continue reading remaining output
                                            handler_exit_code = Some(exit_code);
                                            continue;
                                        }
                                        Ok(Err(e)) => {
                                            // Handler failed - save error and continue draining output
                                            handler_error = Some(e.to_string());
                                            continue;
                                        }
                                        Err(e) => {
                                            // Task panicked - save error and continue draining output
                                            handler_error = Some(format!("Task panicked: {}", e));
                                            continue;
                                        }
                                    }
                                }
                            }
                        };

                        // If streaming failed (connection closed), wait for handler to finish
                        if stream_result.is_err() {
                            cancel_token.cancel();
                            // Wait for handler to finish with timeout (if it hasn't completed yet)
                            if let Some(mut task) = handler_task {
                                select! {
                                    _ = &mut task => {
                                        tracing::debug!("Handler completed after cancellation");
                                    }
                                    _ = sleep(Duration::from_secs(1)) => {
                                        // Handler didn't respect cancellation token - force abort
                                        tracing::warn!("Handler did not respect cancellation token within 1s, forcefully aborting task");
                                        task.abort();
                                    }
                                }
                            }
                        }
                    }
                    .instrument(tracing::info_span!("client", id = client_id)),
                );
                }
                Some(Err(e)) => {
                    // Try to downcast to std::io::Error to inspect error kind
                    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                        match io_err.kind() {
                            // Transient errors - log and continue with backoff
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                                tracing::warn!(error = %e, "Transient accept error, retrying after backoff");
                                sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                            // Connection errors during accept - log and continue with backoff
                            ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset => {
                                tracing::warn!(error = %e, "Connection error during accept, continuing");
                                sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                            // Fatal errors - log and break
                            _ => {
                                tracing::error!(error = %e, kind = ?io_err.kind(), "Fatal accept error, shutting down");
                                break;
                            }
                        }
                    } else {
                        // Non-IO error - treat as fatal
                        tracing::error!(error = %e, "Non-IO accept error, shutting down");
                        break;
                    }
                }
                None => {
                    // Should not happen with current logic
                    break;
                }
            }
        }

        Ok(())
    }
}

// #[cfg(test)]
// #[path = "server_tests.rs"]
// mod tests;
