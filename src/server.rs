use crate::transport::{SocketMessage, SocketServer};
use crate::*;
use anyhow::Result;
use std::{fs, process, time::Duration};
use tokio::{io::AsyncReadExt, select, spawn, sync::oneshot, time::sleep};

/// Daemon server that processes commands from CLI clients.
///
/// Handles one command at a time with streaming output,
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
    /// Create a new daemon server instance.
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
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = Self {
            daemon_id,
            build_timestamp,
            handler,
            shutdown_rx,
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
    /// - Accepts one client connection at a time
    /// - Performs version handshake on connection
    /// - Streams output as it's generated
    /// - Handles task cancellation via connection close detection
    /// - Shuts down gracefully if shutdown signal is received
    pub async fn run(mut self) -> Result<()> {
        let mut socket_server = SocketServer::new(self.daemon_id).await?;

        // Write PID file for precise process management
        let pid = process::id();
        let pid_file = crate::transport::pid_path(self.daemon_id);
        if let Err(e) = fs::write(&pid_file, pid.to_string()) {
            eprintln!("Warning: Failed to write PID file: {}", e);
        }

        // Ensure PID file cleanup on exit
        let cleanup_pid_file = pid_file.clone();
        let _cleanup_guard = scopeguard::guard((), move |_| {
            let _ = fs::remove_file(&cleanup_pid_file);
        });

        println!(
            "Daemon {} listening on socket: {:?}",
            self.daemon_id,
            socket_server.socket_path()
        );

        loop {
            // Select between accepting connection and shutdown signal
            let accept_result = select! {
                result = socket_server.accept() => Some(result),
                _ = &mut self.shutdown_rx => {
                    println!("Daemon {} received shutdown signal", self.daemon_id);
                    break;
                }
            };

            match accept_result {
                Some(Ok(mut connection)) => {
                    let handler = self.handler.clone();
                    let build_timestamp = self.build_timestamp;

                    // Handle this client connection
                    spawn(async move {
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
                                return;
                            }

                            // If versions don't match, client will restart us - just wait for disconnect
                            if client_timestamp != build_timestamp {
                                return;
                            }
                        } else {
                            // No version check received
                            return;
                        }

                        // Receive command
                        let command = match connection.receive_message::<SocketMessage>().await {
                            Ok(Some(SocketMessage::Command(cmd))) => cmd,
                            _ => return,
                        };

                        // Create a pipe for streaming output
                        let (output_writer, mut output_reader) = tokio::io::duplex(8192);

                        // Create cancellation token
                        let cancel_token = tokio_util::sync::CancellationToken::new();
                        let cancel_token_clone = cancel_token.clone();

                        // Spawn handler task
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
                                                let _ = connection.send_message(&SocketMessage::CommandError(error.clone())).await;
                                                let _ = connection.flush().await;
                                                Err(anyhow::anyhow!("{}", error))
                                            } else {
                                                let _ = connection.send_message(&SocketMessage::CommandComplete).await;
                                                let _ = connection.flush().await;
                                                Ok(())
                                            };

                                            // Wait for client to close connection (or timeout)
                                            // This ensures the message is received before connection closes
                                            let _ = connection.receive_message::<SocketMessage>().await;

                                            break result;
                                        }
                                        Ok(n) => {
                                            // Send chunk to client
                                            let chunk = buffer[..n].to_vec();
                                            if connection.send_message(&SocketMessage::OutputChunk(chunk)).await.is_err() {
                                                // Connection closed - cancel handler
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
                            if let Some(task) = handler_task {
                                select! {
                                    _ = task => {}
                                    _ = sleep(Duration::from_secs(1)) => {
                                        // Force abort if handler doesn't finish in time
                                    }
                                }
                            }
                        }
                    });
                }
                Some(Err(e)) => {
                    eprintln!("Failed to accept connection: {e}");
                    break;
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
