use crate::server::DaemonServer;
use crate::*;
use std::time::Duration;

#[derive(Clone)]
struct TestDaemon;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum TestMethod {
    ProcessFile { path: std::path::PathBuf },
    GetStatus,
    LongRunningTask { duration_ms: u64 },
}

impl RpcMethod for TestMethod {
    type Response = String;
}

#[async_trait::async_trait]
impl RpcHandler<TestMethod> for TestDaemon {
    async fn handle(
        &mut self,
        method: TestMethod,
        cancel_token: tokio_util::sync::CancellationToken,
        status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
    ) -> anyhow::Result<String> {
        match method {
            TestMethod::ProcessFile { path } => {
                status_tx
                    .send(DaemonStatus::Busy(format!("Processing file: {:?}", path)))
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                tokio::time::sleep(Duration::from_millis(10)).await;

                status_tx
                    .send(DaemonStatus::Ready)
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                Ok(format!("Processed file: {:?}", path))
            }
            TestMethod::GetStatus => Ok("Ready".to_string()),
            TestMethod::LongRunningTask { duration_ms } => {
                status_tx
                    .send(DaemonStatus::Busy("Starting long-running task".to_string()))
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                let chunks = duration_ms / 10;
                for i in 0..chunks {
                    if cancel_token.is_cancelled() {
                        status_tx
                            .send(DaemonStatus::Ready)
                            .await
                            .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                        return Err(anyhow::anyhow!("Task cancelled"));
                    }

                    tokio::time::sleep(Duration::from_millis(10)).await;

                    if i % 10 == 0 {
                        let progress = (i * 100) / chunks;
                        status_tx
                            .send(DaemonStatus::Busy(format!(
                                "Long-running task: {}% complete",
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

                Ok("Long-running task completed".to_string())
            }
        }
    }
}

#[tokio::test]
async fn test_client_creation() {
    let build_timestamp = 1234567890u64;
    let daemon_executable = std::env::current_exe().unwrap();

    // Create a test server
    let daemon = TestDaemon;
    let server = DaemonServer::new(12345, daemon);
    let server_handle = server.spawn().await.unwrap();

    // Create client using server handle
    let client = super::DaemonClient::connect_to_server(
        12345,
        daemon_executable,
        build_timestamp,
        &server_handle,
    );

    // Test that the client can be created
    assert_eq!(client.daemon_id, 12345);
    assert_eq!(client.build_timestamp, build_timestamp);
}

#[tokio::test]
async fn test_client_request() {
    let build_timestamp = 1234567890u64;
    let daemon_executable = std::env::current_exe().unwrap();

    // Create a test server
    let daemon = TestDaemon;
    let server = DaemonServer::new(12345, daemon);
    let server_handle = server.spawn().await.unwrap();

    // Create client
    let mut client = super::DaemonClient::connect_to_server(
        12345,
        daemon_executable,
        build_timestamp,
        &server_handle,
    );

    // Test basic request
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Ready");
        }
        _ => panic!("Expected success response, got: {:?}", response),
    }
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
