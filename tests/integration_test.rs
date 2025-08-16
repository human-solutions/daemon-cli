use std::path::PathBuf;
use std::time::Duration;
use daemon_rpc::prelude::*;

#[derive(Serialize, Deserialize, Debug)]
enum TestMethod {
    ProcessFile { path: PathBuf },
    GetStatus,
    LongRunningTask { duration_ms: u64 },
}

impl RpcMethod for TestMethod {
    type Response = String;
}

struct TestDaemon;

#[async_trait]
impl RpcHandler<TestMethod> for TestDaemon {
    async fn handle(
        &mut self,
        method: TestMethod,
        cancel_token: CancellationToken,
        status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
    ) -> Result<String> {
        match method {
            TestMethod::ProcessFile { path } => {
                status_tx.send(DaemonStatus::Busy(format!("Processing file: {:?}", path))).await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                
                tokio::time::sleep(Duration::from_millis(10)).await;
                
                status_tx.send(DaemonStatus::Ready).await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                
                Ok(format!("Processed file: {:?}", path))
            },
            TestMethod::GetStatus => {
                Ok("Ready".to_string())
            },
            TestMethod::LongRunningTask { duration_ms } => {
                status_tx.send(DaemonStatus::Busy("Starting long-running task".to_string())).await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                let chunks = duration_ms / 10;
                for i in 0..chunks {
                    if cancel_token.is_cancelled() {
                        status_tx.send(DaemonStatus::Ready).await
                            .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                        return Err(anyhow::anyhow!("Task cancelled"));
                    }

                    tokio::time::sleep(Duration::from_millis(10)).await;

                    if i % 10 == 0 {
                        let progress = (i * 100) / chunks;
                        status_tx.send(DaemonStatus::Busy(format!("Long-running task: {}% complete", progress))).await
                            .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                    }
                }

                status_tx.send(DaemonStatus::Ready).await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                
                Ok("Long-running task completed".to_string())
            },
        }
    }
}


#[tokio::test]
async fn test_daemon_client_creation() {
    let build_timestamp = 1234567890u64;
    let daemon_executable = std::env::current_exe().unwrap();
    
    let client: Result<DaemonClient<TestMethod>> = DaemonClient::connect(
        12345,
        daemon_executable,
        build_timestamp,
    ).await;

    // Test that the client can be created
    assert!(client.is_ok());
    let client = client.unwrap();
    assert_eq!(client.daemon_id, 12345);
    assert_eq!(client.build_timestamp, build_timestamp);
}

#[tokio::test]
async fn test_daemon_server_creation() {
    let daemon = TestDaemon;
    let server = DaemonServer::new(12345, daemon);
    
    // Test that server can be created (actual run() is todo!)
    assert_eq!(server.daemon_id, 12345);
}

#[tokio::test]
async fn test_daemon_client_methods() {
    let build_timestamp = 1234567890u64;
    let daemon_executable = std::env::current_exe().unwrap();
    
    let client: DaemonClient<TestMethod> = DaemonClient::connect(
        12345,
        daemon_executable,
        build_timestamp,
    ).await.unwrap();

    // Test that client has the expected fields
    assert_eq!(client.daemon_id, 12345);
    assert_eq!(client.build_timestamp, build_timestamp);
}

#[tokio::test]
async fn test_rpc_structures() {
    let method = TestMethod::ProcessFile { 
        path: "/tmp/test.txt".into() 
    };
    
    let request = RpcRequest {
        method,
        client_build_timestamp: 1234567890,
    };

    // Test serialization
    let serialized = serde_json::to_string(&request).unwrap();
    let deserialized: RpcRequest<TestMethod> = serde_json::from_str(&serialized).unwrap();
    
    assert_eq!(deserialized.client_build_timestamp, 1234567890);

    // Test response types
    let success_response: RpcResponse<String> = RpcResponse::Success { 
        output: "test".to_string() 
    };
    let error_response: RpcResponse<String> = RpcResponse::Error { 
        error: "test error".to_string() 
    };
    let version_mismatch: RpcResponse<String> = RpcResponse::VersionMismatch { 
        daemon_build_timestamp: 9876543210 
    };

    // Test that responses can be serialized
    assert!(serde_json::to_string(&success_response).is_ok());
    assert!(serde_json::to_string(&error_response).is_ok());
    assert!(serde_json::to_string(&version_mismatch).is_ok());
}

#[tokio::test]
async fn test_daemon_status() {
    let status_ready = DaemonStatus::Ready;
    let status_busy = DaemonStatus::Busy("Processing".to_string());
    let status_error = DaemonStatus::Error("Failed".to_string());

    // Test serialization
    assert!(serde_json::to_string(&status_ready).is_ok());
    assert!(serde_json::to_string(&status_busy).is_ok());
    assert!(serde_json::to_string(&status_error).is_ok());
}