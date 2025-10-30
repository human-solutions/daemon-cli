use anyhow::Result;
use daemon_cli::prelude::*;
use rand::Rng;
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    spawn,
    task::JoinHandle,
    time::sleep,
};

// Helper to generate a random daemon ID for test isolation
fn generate_test_daemon_id() -> u64 {
    rand::thread_rng().gen_range(10000..99999)
}

// Helper to start a daemon server with cleanup
async fn start_test_daemon<H: CommandHandler + Clone + 'static>(
    daemon_id: u64,
    build_timestamp: u64,
    handler: H,
) -> (DaemonHandle, JoinHandle<()>) {
    let (server, shutdown_handle) = DaemonServer::new(daemon_id, build_timestamp, handler);
    let join_handle = spawn(async move {
        server.run().await.ok();
    });

    // Wait for server to start
    sleep(Duration::from_millis(100)).await;

    (shutdown_handle, join_handle)
}

// Helper to cleanup daemon server
async fn stop_test_daemon(shutdown_handle: DaemonHandle, join_handle: JoinHandle<()>) {
    shutdown_handle.shutdown();
    // Wait for server to finish with timeout
    let _ = tokio::time::timeout(Duration::from_secs(2), join_handle).await;
}

// Test handler that echoes commands
#[derive(Clone)]
struct EchoHandler;

#[async_trait]
impl CommandHandler for EchoHandler {
    async fn handle(
        &self,
        command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        _cancel: CancellationToken,
    ) -> Result<()> {
        output.write_all(b"Echo: ").await?;
        output.write_all(command.as_bytes()).await?;
        output.write_all(b"\n").await?;
        Ok(())
    }
}

// Test handler that produces multiple chunks
#[derive(Clone)]
struct ChunkedHandler;

#[async_trait]
impl CommandHandler for ChunkedHandler {
    async fn handle(
        &self,
        _command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        _cancel: CancellationToken,
    ) -> Result<()> {
        for i in 1..=5 {
            output
                .write_all(format!("Chunk {}\n", i).as_bytes())
                .await?;
            sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
}

// Test handler that respects cancellation
#[derive(Clone)]
struct CancellableHandler;

#[async_trait]
impl CommandHandler for CancellableHandler {
    async fn handle(
        &self,
        _command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        cancel: CancellationToken,
    ) -> Result<()> {
        for i in 1..=100 {
            if cancel.is_cancelled() {
                output.write_all(b"Cancelled\n").await?;
                return Err(anyhow::anyhow!("Task was cancelled"));
            }
            output.write_all(format!("Step {}\n", i).as_bytes()).await?;
            sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }
}

// Test handler that produces an error
#[derive(Clone)]
struct ErrorHandler;

#[async_trait]
impl CommandHandler for ErrorHandler {
    async fn handle(
        &self,
        _command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        _cancel: CancellationToken,
    ) -> Result<()> {
        output.write_all(b"Starting...\n").await?;
        Err(anyhow::anyhow!("Test error"))
    }
}

#[tokio::test]
async fn test_basic_streaming() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567890;
    let handler = EchoHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    // Connect client (note: this would normally auto-spawn, but we started manually)
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect(daemon_id, daemon_exe, build_timestamp).await?;

    // Execute command and capture output
    let result = client.execute_command("Hello, World!".to_string()).await;

    // Note: execute_command writes to stdout, so we can't easily capture it in this test
    // In a real integration test, we'd redirect stdout or use a different approach
    assert!(result.is_ok());

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_chunked_output() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567891;
    let handler = ChunkedHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    // Connect and execute
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect(daemon_id, daemon_exe, build_timestamp).await?;

    let result = client.execute_command("test".to_string()).await;
    assert!(result.is_ok());

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_handler_error_reporting() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567892;
    let handler = ErrorHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    // Connect and execute
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect(daemon_id, daemon_exe, build_timestamp).await?;

    let result = client.execute_command("test".to_string()).await;

    // Should get an error
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Test error"));

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_multiple_sequential_commands() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567893;
    let handler = EchoHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Execute multiple commands sequentially
    for i in 1..=3 {
        let mut client =
            DaemonClient::connect(daemon_id, daemon_exe.clone(), build_timestamp).await?;
        let result = client.execute_command(format!("Command {}", i)).await;
        assert!(result.is_ok());

        // Small delay between commands
        sleep(Duration::from_millis(50)).await;
    }

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_connection_close_during_processing() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567894;
    let handler = CancellableHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    // Connect and start long-running command
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect(daemon_id, daemon_exe, build_timestamp).await?;

    // Start the command and then drop the client to simulate connection close
    let command_handle =
        spawn(async move { client.execute_command("long task".to_string()).await });

    // Let it run for a bit
    sleep(Duration::from_millis(200)).await;

    // Abort the task (simulates connection close)
    command_handle.abort();

    // Wait a bit for cleanup
    sleep(Duration::from_millis(100)).await;

    // Server should have cleaned up and be ready for new connections
    // This is tested implicitly - if the server hung, the next test would fail

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}
