use crate::transport::{SocketMessage, SocketServer};
use crate::*;
use anyhow::Result;
use std::{
    fs,
    io::ErrorKind,
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
/// cancellation support, and version checking.
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
///         mut output: impl AsyncWrite + Send + Unpin,
///         _cancel_token: CancellationToken,
///     ) -> Result<()> {
///         output.write_all(b"Processed: ").await?;
///         output.write_all(command.as_bytes()).await?;
///         Ok(())
///     }
/// }
///
/// // Demonstrate server creation
/// let daemon = MyDaemon;
/// let (server, _handle) = DaemonServer::new(1000, 1234567890, daemon);
/// // Use handle.shutdown() to stop the server, or drop it to run indefinitely
/// ```
pub struct DaemonServer<H> {
    /// Unique identifier for this daemon instance
    pub daemon_id: u64,
    /// Build timestamp for version compatibility checking
    pub build_timestamp: u64,
    handler: H,
    shutdown_rx: oneshot::Receiver<()>,
    connection_semaphore: Arc<Semaphore>,
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

impl<H> DaemonServer<H>
where
    H: CommandHandler + Clone + 'static,
{
    /// Create a new daemon server instance with default connection limit (100).
    ///
    /// Returns the server and a handle that can be used to shut it down gracefully.
    ///
    /// # Parameters
    ///
    /// * `daemon_id` - Unique identifier for this daemon instance
    /// * `build_timestamp` - Build timestamp for version compatibility checking
    /// * `handler` - Your command handler implementation
    ///
    /// # Returns
    ///
    /// A tuple of (DaemonServer, DaemonHandle). Call `shutdown()` on the handle
    /// to gracefully stop the server, or drop it to let the server run indefinitely.
    ///
    pub fn new(daemon_id: u64, build_timestamp: u64, handler: H) -> (Self, DaemonHandle) {
        Self::new_with_limit(daemon_id, build_timestamp, handler, 100)
    }

    /// Create a new daemon server instance with custom connection limit.
    ///
    /// Returns the server and a handle that can be used to shut it down gracefully.
    ///
    /// # Parameters
    ///
    /// * `daemon_id` - Unique identifier for this daemon instance
    /// * `build_timestamp` - Build timestamp for version compatibility checking
    /// * `handler` - Your command handler implementation
    /// * `max_connections` - Maximum number of concurrent client connections
    ///
    /// # Returns
    ///
    /// A tuple of (DaemonServer, DaemonHandle). Call `shutdown()` on the handle
    /// to gracefully stop the server, or drop it to let the server run indefinitely.
    ///
    /// # Example
    ///
    /// ```rust
    /// use daemon_cli::prelude::*;
    /// # use tokio::io::{AsyncWrite, AsyncWriteExt};
    /// #
    /// # #[derive(Clone)]
    /// # struct MyHandler;
    /// #
    /// # #[async_trait]
    /// # impl CommandHandler for MyHandler {
    /// #     async fn handle(
    /// #         &self,
    /// #         command: &str,
    /// #         mut output: impl AsyncWrite + Send + Unpin,
    /// #         _cancel_token: CancellationToken,
    /// #     ) -> Result<()> {
    /// #         output.write_all(command.as_bytes()).await?;
    /// #         Ok(())
    /// #     }
    /// # }
    ///
    /// let handler = MyHandler;
    /// let (server, _handle) = DaemonServer::new_with_limit(1000, 1234567890, handler, 10);
    /// ```
    pub fn new_with_limit(
        daemon_id: u64,
        build_timestamp: u64,
        handler: H,
        max_connections: usize,
    ) -> (Self, DaemonHandle) {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Create semaphore for connection limiting
        let connection_semaphore = Arc::new(Semaphore::new(max_connections));

        let server = Self {
            daemon_id,
            build_timestamp,
            handler,
            shutdown_rx,
            connection_semaphore,
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
    /// - Creates Unix socket at `/tmp/daemon-cli-{daemon_id}.sock`
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
    /// thread-safe if they access shared mutable state (use Arc<Mutex<T>> or
    /// similar synchronization primitives).
    pub async fn run(mut self) -> Result<()> {
        let mut socket_server = SocketServer::new(self.daemon_id).await?;

        // Write PID file for precise process management
        let pid = process::id();
        let pid_file = crate::transport::pid_path(self.daemon_id);
        if let Err(e) = fs::write(&pid_file, pid.to_string()) {
            tracing::warn!(pid_file = ?pid_file, error = %e, "Failed to write PID file");
        }

        // Ensure PID file cleanup on exit
        let cleanup_pid_file = pid_file.clone();
        let _cleanup_guard = scopeguard::guard((), move |_| {
            let _ = fs::remove_file(&cleanup_pid_file);
        });

        tracing::info!(
            daemon_id = self.daemon_id,
            socket_path = ?socket_server.socket_path(),
            build_timestamp = self.build_timestamp,
            "Daemon started and listening"
        );

        loop {
            // Select between accepting connection and shutdown signal
            let accept_result = select! {
                result = socket_server.accept() => Some(result),
                _ = &mut self.shutdown_rx => {
                    tracing::info!(daemon_id = self.daemon_id, "Daemon received shutdown signal");
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
                        if let Ok(Some(SocketMessage::VersionCheck {
                            build_timestamp: client_timestamp,
                        })) = connection.receive_message().await
                        {
                            // Send our build timestamp
                            if connection
                                .send_message(&SocketMessage::VersionCheck { build_timestamp })
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
                        let command = match connection.receive_message::<SocketMessage>().await {
                            Ok(Some(SocketMessage::Command(cmd))) => cmd,
                            _ => {
                                tracing::warn!("No command received from client");
                                return;
                            }
                        };

                        tracing::debug!("Received command");

                        // Create a pipe for streaming output
                        let (output_writer, mut output_reader) = tokio::io::duplex(8192);

                        // Create cancellation token
                        let cancel_token = tokio_util::sync::CancellationToken::new();
                        let cancel_token_clone = cancel_token.clone();

                        // Spawn handler task (will not inherit span by default)
                        let mut handler_task = Some(spawn(async move {
                            handler
                                .handle(&command, output_writer, cancel_token_clone)
                                .await
                        }));

                        // Stream output chunks to client
                        let mut buffer = vec![0u8; 4096];
                        let mut handler_error: Option<String> = None;
                        let stream_result = loop {
                            select! {
                                // Read from handler output
                                read_result = output_reader.read(&mut buffer) => {
                                    match read_result {
                                        Ok(0) => {
                                            // CRITICAL FIX: If handler task hasn't been polled yet, check it now
                                            // This ensures we capture any error before sending completion message
                                            if let Some(task) = handler_task.take() {
                                                match task.await {
                                                    Ok(Err(e)) => {
                                                        handler_error = Some(e.to_string());
                                                    }
                                                    Err(e) => {
                                                        handler_error = Some(format!("Task panicked: {}", e));
                                                    }
                                                    _ => {}
                                                }
                                            }

                                            // EOF - handler closed output
                                            // Send completion message (error or success)
                                            let result = if let Some(ref error) = handler_error {
                                                tracing::error!(error = %error, "Handler failed");
                                                let _ = connection.send_message(&SocketMessage::CommandError(error.clone())).await;
                                                let _ = connection.flush().await;
                                                Err(anyhow::anyhow!("{}", error))
                                            } else {
                                                tracing::debug!("Handler completed successfully");
                                                let _ = connection.send_message(&SocketMessage::CommandComplete).await;
                                                let _ = connection.flush().await;
                                                Ok(())
                                            };

                                            // Wait for client to close connection with timeout
                                            // This ensures the message is received before connection closes
                                            let _ = tokio::time::timeout(
                                                Duration::from_secs(5),
                                                connection.receive_message::<SocketMessage>()
                                            ).await;

                                            break result;
                                        }
                                        Ok(n) => {
                                            // Send chunk to client
                                            let chunk = buffer[..n].to_vec();
                                            if connection.send_message(&SocketMessage::OutputChunk(chunk)).await.is_err() {
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
                                        Ok(Ok(())) => {
                                            // Handler succeeded - continue reading remaining output
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
