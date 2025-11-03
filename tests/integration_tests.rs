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

// Helper to generate unique daemon name and path for test isolation
fn generate_test_daemon_config() -> (String, String) {
    let test_id: u64 = rand::thread_rng().gen_range(10000..99999);
    let daemon_name = format!("test-{}", test_id);
    let root_path = format!("/tmp/daemon-test-{}", test_id);
    (daemon_name, root_path)
}

// Helper to start a daemon server with cleanup
async fn start_test_daemon<H: CommandHandler + Clone + 'static>(
    daemon_name: &str,
    root_path: &str,
    build_timestamp: u64,
    handler: H,
) -> (DaemonHandle, JoinHandle<()>) {
    let (server, shutdown_handle) = DaemonServer::new_with_name_and_timestamp(daemon_name, root_path, build_timestamp, handler, 100);
    let join_handle = spawn(async move {
        server.run().await.ok();
    });

    // Wait for server to start
    sleep(Duration::from_millis(100)).await;

    (shutdown_handle, join_handle)
}

// Helper to start a daemon server with custom connection limit
async fn start_test_daemon_with_limit<H: CommandHandler + Clone + 'static>(
    daemon_name: &str,
    root_path: &str,
    build_timestamp: u64,
    handler: H,
    max_connections: usize,
) -> (DaemonHandle, JoinHandle<()>) {
    let (server, shutdown_handle) =
        DaemonServer::new_with_name_and_timestamp(daemon_name, root_path, build_timestamp, handler, max_connections);
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
    ) -> Result<i32> {
        output.write_all(b"Echo: ").await?;
        output.write_all(command.as_bytes()).await?;
        output.write_all(b"\n").await?;
        Ok(0)
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
    ) -> Result<i32> {
        for i in 1..=5 {
            output
                .write_all(format!("Chunk {}\n", i).as_bytes())
                .await?;
            sleep(Duration::from_millis(10)).await;
        }
        Ok(0)
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
    ) -> Result<i32> {
        for i in 1..=100 {
            if cancel.is_cancelled() {
                output.write_all(b"Cancelled\n").await?;
                return Err(anyhow::anyhow!("Task was cancelled"));
            }
            output.write_all(format!("Step {}\n", i).as_bytes()).await?;
            sleep(Duration::from_millis(50)).await;
        }
        Ok(0)
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
    ) -> Result<i32> {
        output.write_all(b"Starting...\n").await?;
        Err(anyhow::anyhow!("Test error"))
    }
}

#[tokio::test]
async fn test_basic_streaming() -> Result<()> {
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567890;
    let handler = EchoHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    // Connect client (note: this would normally auto-spawn, but we started manually)
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect_with_name_and_timestamp(&daemon_name, &root_path, daemon_exe, build_timestamp).await?;

    // Execute command and capture exit code
    let result = client.execute_command("Hello, World!".to_string()).await;

    // Note: execute_command writes to stdout, so we can't easily capture it in this test
    // In a real integration test, we'd redirect stdout or use a different approach
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);  // Success exit code

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_chunked_output() -> Result<()> {
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567891;
    let handler = ChunkedHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    // Connect and execute
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect_with_name_and_timestamp(&daemon_name, &root_path, daemon_exe, build_timestamp).await?;

    let result = client.execute_command("test".to_string()).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);  // Success exit code

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_handler_error_reporting() -> Result<()> {
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567892;
    let handler = ErrorHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    // Connect and execute
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect_with_name_and_timestamp(&daemon_name, &root_path, daemon_exe, build_timestamp).await?;

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
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567893;
    let handler = EchoHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Execute multiple commands sequentially
    for i in 1..=3 {
        let mut client =
            DaemonClient::connect_with_name_and_timestamp(&daemon_name, &root_path, daemon_exe.clone(), build_timestamp).await?;
        let result = client.execute_command(format!("Command {}", i)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);  // Success exit code

        // Small delay between commands
        sleep(Duration::from_millis(50)).await;
    }

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}

#[tokio::test]
async fn test_connection_close_during_processing() -> Result<()> {
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567894;
    let handler = CancellableHandler;

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    // Connect and start long-running command
    let daemon_exe = PathBuf::from("./target/debug/examples/cli");
    let mut client = DaemonClient::connect_with_name_and_timestamp(&daemon_name, &root_path, daemon_exe, build_timestamp).await?;

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
    ) -> Result<i32> {
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

        Ok(0)
    }
}

#[tokio::test]
async fn test_concurrent_clients() -> Result<()> {
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567895;
    let handler = ConcurrentTrackingHandler::new();
    let max_concurrent_ref = handler.max_concurrent.clone();

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Spawn 5 concurrent clients
    let mut client_handles = vec![];
    for i in 0..5 {
        let daemon_exe_clone = daemon_exe.clone();
        let daemon_name_clone = daemon_name.clone();
        let root_path_clone = root_path.clone();
        let handle = spawn(async move {
            let mut client =
                DaemonClient::connect_with_name_and_timestamp(&daemon_name_clone, &root_path_clone, daemon_exe_clone, build_timestamp).await?;
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
        let exit_code = result.unwrap();
        assert!(exit_code.is_ok());
        assert_eq!(exit_code.unwrap(), 0);  // Success exit code
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
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567896;
    let handler = ConcurrentTrackingHandler::new();
    let max_concurrent_ref = handler.max_concurrent.clone();

    // Start server with cleanup
    let (shutdown_handle, join_handle) =
        start_test_daemon(&daemon_name, &root_path, build_timestamp, handler).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Spawn 15 concurrent clients
    let num_clients = 15;
    let mut client_handles = vec![];
    for i in 0..num_clients {
        let daemon_exe_clone = daemon_exe.clone();
        let daemon_name_clone = daemon_name.clone();
        let root_path_clone = root_path.clone();
        let handle = spawn(async move {
            let mut client =
                DaemonClient::connect_with_name_and_timestamp(&daemon_name_clone, &root_path_clone, daemon_exe_clone, build_timestamp).await?;
            client.execute_command(format!("stress-test-{}", i)).await
        });
        client_handles.push(handle);
    }

    // Wait for all clients to complete
    let mut success_count = 0;
    for handle in client_handles {
        let result = handle.await;
        assert!(result.is_ok(), "Client task panicked");
        match result.unwrap() {
            Ok(exit_code) if exit_code == 0 => success_count += 1,
            Ok(exit_code) => panic!("Unexpected exit code: {}", exit_code),
            Err(_) => {} // Expected errors are ok in stress test
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
    let (daemon_name, root_path) = generate_test_daemon_config();
    let build_timestamp = 1234567897;
    let handler = ConcurrentTrackingHandler::new();
    let max_concurrent_ref = handler.max_concurrent.clone();

    // Start server with connection limit of 3
    let (shutdown_handle, join_handle) =
        start_test_daemon_with_limit(&daemon_name, &root_path, build_timestamp, handler, 3).await;

    let daemon_exe = PathBuf::from("./target/debug/examples/cli");

    // Spawn 6 concurrent clients (more than the limit)
    let num_clients = 6;
    let mut client_handles = vec![];
    for i in 0..num_clients {
        let daemon_exe_clone = daemon_exe.clone();
        let daemon_name_clone = daemon_name.clone();
        let root_path_clone = root_path.clone();
        let handle = spawn(async move {
            let mut client =
                DaemonClient::connect_with_name_and_timestamp(&daemon_name_clone, &root_path_clone, daemon_exe_clone, build_timestamp).await?;
            client.execute_command(format!("limit-test-{}", i)).await
        });
        client_handles.push(handle);
        // Small delay to stagger connections slightly
        sleep(Duration::from_millis(10)).await;
    }

    // Wait for all clients to complete
    let mut success_count = 0;
    let mut rejected_count = 0;
    for handle in client_handles {
        let result = handle.await;
        assert!(result.is_ok(), "Client task panicked");
        match result.unwrap() {
            Ok(exit_code) if exit_code == 0 => success_count += 1,
            Ok(exit_code) => panic!("Unexpected exit code: {}", exit_code),
            Err(e) => {
                // When server is at capacity, connection is dropped which causes various errors
                // (connection reset, unexpected close, invalid handshake, etc.)
                rejected_count += 1;
                println!("Client rejected with error: {}", e);
            }
        }
    }

    // With non-blocking semaphore, connections are rejected when at capacity
    // So we expect some clients to succeed (up to the limit) and some to be rejected
    assert!(
        success_count <= 3,
        "Expected at most 3 successful executions, got {}",
        success_count
    );
    assert!(
        success_count + rejected_count == num_clients,
        "Expected {} total clients (success + rejected), got {}",
        num_clients,
        success_count + rejected_count
    );

    // Verify that we respected the connection limit
    let max_concurrent = *max_concurrent_ref.lock().await;
    assert!(
        max_concurrent <= 3,
        "Expected max 3 concurrent executions, got {}",
        max_concurrent
    );

    println!(
        "Connection limit test: {} clients, {} succeeded, {} rejected, max {} concurrent (limit was 3)",
        num_clients, success_count, rejected_count, max_concurrent
    );

    // Cleanup
    stop_test_daemon(shutdown_handle, join_handle).await;

    Ok(())
}
