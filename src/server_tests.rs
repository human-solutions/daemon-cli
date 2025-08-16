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
async fn test_server_creation() {
    let daemon = TestDaemon;
    let server = super::DaemonServer::new(12345, daemon);
    assert_eq!(server.daemon_id, 12345);
}

#[tokio::test]
async fn test_single_task_enforcement() {
    use crate::client::DaemonClient;

    // Create server and get handle
    let daemon = TestDaemon;
    let server = super::DaemonServer::new(12345, daemon);
    let server_handle = server.spawn().await.unwrap();

    // Create two clients
    let mut client1 = DaemonClient::connect_to_server(
        12345,
        std::env::current_exe().unwrap(),
        1234567890,
        &server_handle,
    );
    let mut client2 = DaemonClient::connect_to_server(
        12345,
        std::env::current_exe().unwrap(),
        1234567890,
        &server_handle,
    );

    // Start a long-running task on client1
    let client1_task = tokio::spawn(async move {
        client1
            .request(TestMethod::LongRunningTask { duration_ms: 500 })
            .await
    });

    // Give client1 time to start and get into busy state
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Try to send request from client2 - should fail with busy
    let response2 = client2.request(TestMethod::GetStatus).await.unwrap();
    match response2 {
        RpcResponse::Error { error } => {
            assert!(
                error.contains("busy"),
                "Expected busy error, got: {}",
                error
            );
        }
        _ => panic!("Expected error response, got: {:?}", response2),
    }

    // Wait for client1 to complete
    let response1 = client1_task.await.unwrap().unwrap();
    match response1 {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Long-running task completed");
        }
        _ => panic!("Expected success response, got: {:?}", response1),
    }

    // Now client2 should be able to send requests
    let response3 = client2.request(TestMethod::GetStatus).await.unwrap();
    match response3 {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Ready");
        }
        _ => panic!("Expected success response, got: {:?}", response3),
    }
}

#[tokio::test]
async fn test_status_streaming() {
    use crate::client::DaemonClient;

    // Create a daemon that sends status updates
    #[derive(Clone)]
    struct StatusTestDaemon;

    #[async_trait::async_trait]
    impl RpcHandler<TestMethod> for StatusTestDaemon {
        async fn handle(
            &mut self,
            method: TestMethod,
            _cancel_token: tokio_util::sync::CancellationToken,
            status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
        ) -> anyhow::Result<String> {
            match method {
                TestMethod::LongRunningTask { duration_ms } => {
                    // Send multiple status updates during execution
                    status_tx
                        .send(DaemonStatus::Busy("Task started".to_string()))
                        .await
                        .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                    let steps = 5;
                    let step_duration = duration_ms / steps;

                    for i in 1..=steps {
                        tokio::time::sleep(Duration::from_millis(step_duration)).await;

                        let progress = (i * 100) / steps;
                        status_tx
                            .send(DaemonStatus::Busy(format!("Progress: {}%", progress)))
                            .await
                            .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                    }

                    status_tx
                        .send(DaemonStatus::Ready)
                        .await
                        .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                    Ok("Status streaming task completed".to_string())
                }
                _ => Ok("Other method".to_string()),
            }
        }
    }

    // Create server with status test daemon
    let daemon = StatusTestDaemon;
    let server = super::DaemonServer::new(12345, daemon);
    let server_handle = server.spawn().await.unwrap();

    // Create multiple clients to test that they all receive status updates
    let mut client1 = DaemonClient::connect_to_server(
        12345,
        std::env::current_exe().unwrap(),
        1234567890,
        &server_handle,
    );
    let mut client2 = DaemonClient::connect_to_server(
        12345,
        std::env::current_exe().unwrap(),
        1234567890,
        &server_handle,
    );

    // Collect status updates from client1
    let client1_status_task = tokio::spawn(async move {
        let mut statuses: Vec<DaemonStatus> = Vec::new();
        while let Ok(status) = client1.status_receiver.recv().await {
            statuses.push(status.clone());
            // Stop collecting after seeing Ready status
            if matches!(status, DaemonStatus::Ready) && statuses.len() > 1 {
                break;
            }
        }
        statuses
    });

    // Collect status updates from client2
    let client2_status_task = tokio::spawn(async move {
        let mut statuses: Vec<DaemonStatus> = Vec::new();
        while let Ok(status) = client2.status_receiver.recv().await {
            statuses.push(status.clone());
            // Stop collecting after seeing Ready status
            if matches!(status, DaemonStatus::Ready) && statuses.len() > 1 {
                break;
            }
        }
        statuses
    });

    // Give clients time to start listening
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Start a task that will generate status updates
    let request_task = tokio::spawn(async move {
        let mut request_client = DaemonClient::connect_to_server(
            12345,
            std::env::current_exe().unwrap(),
            1234567890,
            &server_handle,
        );
        request_client
            .request(TestMethod::LongRunningTask { duration_ms: 500 })
            .await
    });

    // Wait for the task to complete
    let result = request_task.await.unwrap().unwrap();

    // Verify the task completed successfully
    match result {
        RpcResponse::Success { output } => {
            assert_eq!(output, "Status streaming task completed");
        }
        _ => panic!("Expected success response, got: {:?}", result),
    }

    // Give status tasks a moment to finish collecting
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Collect the status updates
    let client1_statuses = client1_status_task.await.unwrap();
    let client2_statuses = client2_status_task.await.unwrap();

    // Verify that both clients received status updates
    assert!(
        client1_statuses.len() >= 3,
        "Client1 should receive multiple status updates, got: {:?}",
        client1_statuses
    );
    assert!(
        client2_statuses.len() >= 3,
        "Client2 should receive multiple status updates, got: {:?}",
        client2_statuses
    );

    // Verify the sequence includes the expected status messages
    assert!(
        matches!(client1_statuses[0], DaemonStatus::Ready),
        "Should start with Ready"
    );

    // Should have some Busy statuses
    let busy_statuses: Vec<_> = client1_statuses
        .iter()
        .filter(|s| matches!(s, DaemonStatus::Busy(_)))
        .collect();
    assert!(
        busy_statuses.len() >= 2,
        "Should have multiple busy status updates, got: {:?}",
        client1_statuses
    );

    // Should end with Ready
    assert!(
        matches!(client1_statuses.last(), Some(DaemonStatus::Ready)),
        "Should end with Ready"
    );

    // Verify both clients got similar updates (broadcast works)
    assert_eq!(
        client1_statuses.len(),
        client2_statuses.len(),
        "Both clients should receive the same number of updates"
    );
}
