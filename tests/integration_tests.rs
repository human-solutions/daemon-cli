use anyhow::Result;
use daemon_cli::prelude::*;
use rand::Rng;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    spawn,
    sync::Mutex,
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

// Helper to start a daemon server with custom connection limit
async fn start_test_daemon_with_limit<H: CommandHandler + Clone + 'static>(
    daemon_id: u64,
    build_timestamp: u64,
    handler: H,
    max_connections: usize,
) -> (DaemonHandle, JoinHandle<()>) {
    let (server, shutdown_handle) = DaemonServer::new_with_limit(daemon_id, build_timestamp, handler, max_connections);
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

// Test handler that tracks concurrent execution
#[derive(Clone)]
struct ConcurrentTrackingHandler {
    active_count: Arc<Mutex<usize>>,
    max_concurrent: Arc<Mutex<usize>>,
}

impl ConcurrentTrackingHandler {
    fn new() -> Self {
        Self {
            active_count: Arc::new(Mutex::new(0)),
            max_concurrent: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl CommandHandler for ConcurrentTrackingHandler {
    async fn handle(
        &self,
        _command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        _cancel: CancellationToken,
    ) -> Result<()> {
        // Increment active count
        {
            let mut active = self.active_count.lock().await;
            *active += 1;

            // Update max concurrent
            let mut max = self.max_concurrent.lock().await;
            if *active > *max {
                *max = *active;
            }
        }

        // Simulate some work
        output.write_all(b"Working...\n").await?;
        sleep(Duration::from_millis(100)).await;
        output.write_all(b"Done\n").await?;

        // Decrement active count
        {
            let mut active = self.active_count.lock().await;
            *active -= 1;
        }

        Ok(())
    }
}

#[tokio::test]
async fn test_concurrent_clients() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567895;
    let handler = ConcurrentTrackingHandler::new();
    let max_concurrent_ref = handler.max_concurrent.clone();

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Spawn 5 concurrent clients
    let mut client_handles = vec![];
    for i in 0..5 {
        let daemon_exe_clone = daemon_exe.clone();
        let handle = spawn(async move {
            let mut client =
                DaemonClient::connect(daemon_id, daemon_exe_clone, build_timestamp).await?;
            client
                .execute_command(format!("concurrent-test-{}", i))
                .await
        });
        client_handles.push(handle);
    }

    // Wait for all clients to complete
    for handle in client_handles {
        let result = handle.await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }

    // Verify that we actually had concurrent execution
    let max_concurrent = *max_concurrent_ref.lock().await;
    assert!(
        max_concurrent >= 2,
        "Expected at least 2 concurrent executions, got {}",
        max_concurrent
    );

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_concurrent_stress_10_plus_clients() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567896;
    let handler = ConcurrentTrackingHandler::new();
    let max_concurrent_ref = handler.max_concurrent.clone();

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(daemon_id, build_timestamp, handler).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Spawn 15 concurrent clients
    let num_clients = 15;
    let mut client_handles = vec![];
    for i in 0..num_clients {
        let daemon_exe_clone = daemon_exe.clone();
        let handle = spawn(async move {
            let mut client =
                DaemonClient::connect(daemon_id, daemon_exe_clone, build_timestamp).await?;
            client
                .execute_command(format!("stress-test-{}", i))
                .await
        });
        client_handles.push(handle);
    }

    // Wait for all clients to complete
    let mut success_count = 0;
    for handle in client_handles {
        let result = handle.await;
        assert!(result.is_ok(), "Client task panicked");
        if result.unwrap().is_ok() {
            success_count += 1;
        }
    }

    // All clients should succeed
    assert_eq!(
        success_count, num_clients,
        "Expected {} successful executions, got {}",
        num_clients, success_count
    );

    // Verify significant concurrent execution
    let max_concurrent = *max_concurrent_ref.lock().await;
    assert!(
        max_concurrent >= 5,
        "Expected at least 5 concurrent executions, got {}",
        max_concurrent
    );

    println!(
        "Stress test: {} clients, max {} concurrent executions",
        num_clients, max_concurrent
    );

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_connection_limit() -> Result<()> {
    let daemon_id = generate_test_daemon_id();
    let build_timestamp = 1234567897;
    let handler = ConcurrentTrackingHandler::new();
    let max_concurrent_ref = handler.max_concurrent.clone();

    // Start server with connection limit of 3
    let (shutdown_handle, join_handle) =
        start_test_daemon_with_limit(daemon_id, build_timestamp, handler, 3).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Spawn 6 concurrent clients (more than the limit)
    let num_clients = 6;
    let mut client_handles = vec![];
    for i in 0..num_clients {
        let daemon_exe_clone = daemon_exe.clone();
        let handle = spawn(async move {
            let mut client =
                DaemonClient::connect(daemon_id, daemon_exe_clone, build_timestamp).await?;
            client
                .execute_command(format!("limit-test-{}", i))
                .await
        });
        client_handles.push(handle);
        // Small delay to stagger connections slightly
        sleep(Duration::from_millis(10)).await;
    }

    // Wait for all clients to complete
    let mut success_count = 0;
    for handle in client_handles {
        let result = handle.await;
        assert!(result.is_ok(), "Client task panicked");
        if result.unwrap().is_ok() {
            success_count += 1;
        }
    }

    // All clients should succeed eventually (they just wait for a permit)
    assert_eq!(
        success_count, num_clients,
        "Expected {} successful executions, got {}",
        num_clients, success_count
    );

    // Verify that we respected the connection limit
    let max_concurrent = *max_concurrent_ref.lock().await;
    assert!(
        max_concurrent <= 3,
        "Expected max 3 concurrent executions, got {}",
        max_concurrent
    );

    println!(
        "Connection limit test: {} clients, max {} concurrent (limit was 3)",
        num_clients, max_concurrent
    );

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}
