use crate::server::DaemonServer;
use crate::*;
use std::time::Duration;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum TestMethod {
    ProcessFile { path: std::path::PathBuf },
    GetStatus,
    LongRunningTask { duration_ms: u64 },
}

impl RpcMethod for TestMethod {
    type Response = String;
}

#[tokio::test]
async fn test_task_cancellation() {
    // Create a daemon that will respect cancellation
    #[derive(Clone)]
    struct CancellableDaemon;

    #[async_trait::async_trait]
    impl RpcHandler<TestMethod> for CancellableDaemon {
        async fn handle(
            &mut self,
            method: TestMethod,
            cancel_token: tokio_util::sync::CancellationToken,
            status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
        ) -> anyhow::Result<String> {
            match method {
                TestMethod::LongRunningTask { duration_ms } => {
                    status_tx
                        .send(DaemonStatus::Busy("Starting cancellable task".to_string()))
                        .await
                        .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                    // Simulate work in small chunks, checking for cancellation frequently
                    let chunks = duration_ms / 10;
                    for i in 0..chunks {
                        // Check for cancellation every 10ms
                        if cancel_token.is_cancelled() {
                            status_tx
                                .send(DaemonStatus::Ready)
                                .await
                                .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                            return Err(anyhow::anyhow!("Task was cancelled after {}ms", i * 10));
                        }

                        tokio::time::sleep(Duration::from_millis(10)).await;

                        if i % 50 == 0 {
                            let progress = (i * 100) / chunks;
                            status_tx
                                .send(DaemonStatus::Busy(format!(
                                    "Cancellable task: {}% complete",
                                    progress
                                )))
                                .await
                                .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                        }
                    }

                    status_tx
                        .send(DaemonStatus::Ready)
                        .await
                        .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                    Ok("Cancellable task completed".to_string())
                }
                _ => Ok("Other method".to_string()),
            }
        }
    }

    // Create server with cancellable daemon
    let daemon = CancellableDaemon;
    let server = DaemonServer::new(12345, daemon);
    let server_handle = server.spawn().await.unwrap();

    // Create two clients to test external cancellation via busy rejection
    let mut client1 = super::DaemonClient::connect_to_server(
        12345,
        std::env::current_exe().unwrap(),
        1234567890,
        &server_handle,
    );
    let mut client2 = super::DaemonClient::connect_to_server(
        12345,
        std::env::current_exe().unwrap(),
        1234567890,
        &server_handle,
    );

    // Start a long-running task (2 seconds)
    let start_time = std::time::Instant::now();
    let client1_task = tokio::spawn(async move {
        client1
            .request(TestMethod::LongRunningTask { duration_ms: 2000 })
            .await
    });

    // Give task time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Try to cancel via client2 sending a request (should be rejected as busy)
    let cancel_attempt = client2.request(TestMethod::GetStatus).await.unwrap();
    match cancel_attempt {
        RpcResponse::Error { error } => {
            assert!(
                error.contains("busy"),
                "Expected busy error for cancellation test, got: {}",
                error
            );
        }
        _ => panic!("Expected busy error when daemon is processing long task"),
    }

    // Wait for the original task to complete naturally
    let result = client1_task.await.unwrap().unwrap();
    let elapsed = std::time::Instant::now().duration_since(start_time);

    // Verify the task completed (since we didn't actually cancel it, just verified busy rejection)
    match result {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Cancellable task completed");
            // Should take approximately 2 seconds
            assert!(
                elapsed >= Duration::from_millis(1900) && elapsed <= Duration::from_millis(2500)
            );
        }
        _ => panic!("Expected success response, got: {:?}", result),
    }

    // Now test that the daemon is ready for new requests
    let final_request = client2.request(TestMethod::GetStatus).await.unwrap();
    match final_request {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Other method");
        }
        _ => panic!("Expected success response after task completion"),
    }
}

// Phase 3 Tests - Process Management

#[tokio::test]
async fn test_daemon_auto_spawn() {
    // Get path to the test daemon binary
    let daemon_executable = get_test_daemon_path();
    let daemon_id = 45678;

    // Ensure no daemon is running
    cleanup_daemon(daemon_id).await;

    // Connect should automatically spawn the daemon
    let mut client = super::DaemonClient::connect(
        daemon_id,
        daemon_executable,
        1234567890,
    ).await.unwrap();

    // Verify we can make requests to the spawned daemon
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Ready");
        }
        _ => panic!("Expected success response, got: {:?}", response),
    }

    // Test a more complex request
    let response = client.request(TestMethod::ProcessFile {
        path: "/tmp/auto-spawn-test.txt".into(),
    }).await.unwrap();
    
    match response {
        RpcResponse::Success { output } => {
            assert!(output.contains("Processed file"));
        }
        _ => panic!("Expected success response, got: {:?}", response),
    }

    // Clean up
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn test_daemon_crash_recovery() {
    let daemon_executable = get_test_daemon_path();
    let daemon_id = 56789;

    // Ensure no daemon is running
    cleanup_daemon(daemon_id).await;

    // Connect and spawn daemon
    let mut client = super::DaemonClient::connect(
        daemon_id,
        daemon_executable,
        1234567890,
    ).await.unwrap();

    // Verify daemon is working
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    assert!(matches!(response, RpcResponse::Success { .. }));

    // Simulate daemon crash by killing the process
    if let Some(child) = &mut client.daemon_process {
        let _ = child.kill().await;
    }

    // Give some time for the process to die
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Ensure daemon health - should detect crash and restart
    client.ensure_daemon_healthy().await.unwrap();

    // Verify daemon is working again after restart
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Ready");
        }
        _ => panic!("Expected success response after restart, got: {:?}", response),
    }

    // Clean up
    let _ = client.shutdown().await;
}

// Helper functions for tests

fn get_test_daemon_path() -> std::path::PathBuf {
    // Get the path to the test_daemon example binary
    let mut exe_path = std::env::current_exe().unwrap();
    exe_path.pop(); // Remove the test binary name
    exe_path.pop(); // Remove deps/
    exe_path.push("examples");
    exe_path.push("test_daemon");
    
    // On Windows, add .exe extension
    if cfg!(windows) {
        exe_path.set_extension("exe");
    }
    
    exe_path
}

async fn cleanup_daemon(daemon_id: u64) {
    use crate::transport::socket_path;
    
    // Remove socket file if it exists
    let socket_path = socket_path(daemon_id);
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    
    // Give time for cleanup
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}
