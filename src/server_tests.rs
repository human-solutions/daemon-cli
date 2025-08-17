use crate::*;
use std::{path::PathBuf, time::Duration};

#[derive(Clone)]
struct TestDaemon;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
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

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap()
}

#[tokio::test]
async fn test_socket_single_task_enforcement() {
    // Create and start socket server
    let daemon = TestDaemon;
    let server = super::DaemonServer::new(23456, 1234567890, daemon);

    // Start server in background task
    let _server_task = tokio::spawn(async move { server.spawn_with_socket().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create two socket clients
    let mut client1 = DaemonClient::connect(23456, exe_path(), 1234567890)
        .await
        .unwrap();

    let mut client2 = crate::client::DaemonClient::connect(23456, exe_path(), 1234567890)
        .await
        .unwrap();

    // Start a long-running task on client1
    let client1_task = tokio::spawn(async move {
        client1
            .request(TestMethod::LongRunningTask { duration_ms: 500 })
            .await
    });

    // Give client1 time to start
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
async fn test_socket_message_framing() {
    // Create and start socket server
    let daemon = TestDaemon;
    let server = super::DaemonServer::new(34567, 1234567890, daemon);

    // Start server in background task
    let _server_task = tokio::spawn(async move { server.spawn_with_socket().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create socket client
    let mut client = crate::client::DaemonClient::connect(34567, exe_path(), 1234567890)
        .await
        .unwrap();

    // Send multiple requests rapidly to test message framing
    for i in 0..10 {
        let response = client
            .request(TestMethod::ProcessFile {
                path: format!("/tmp/test-{}.txt", i).into(),
            })
            .await
            .unwrap();

        match response {
            RpcResponse::Success { output } => {
                assert!(
                    output.contains(&format!("test-{}.txt", i)),
                    "Response should contain correct filename, got: {}",
                    output
                );
            }
            _ => panic!(
                "Expected success response for request {}, got: {:?}",
                i, response
            ),
        }
    }
}
