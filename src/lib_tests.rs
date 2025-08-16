use crate::*;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum TestMethod {
    ProcessFile { path: std::path::PathBuf },
    GetStatus,
}

impl RpcMethod for TestMethod {
    type Response = String;
}

#[test]
fn test_rpc_structures_serialization() {
    let method = TestMethod::ProcessFile {
        path: "/tmp/test.txt".into(),
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
        output: "test".to_string(),
    };
    let error_response: RpcResponse<String> = RpcResponse::Error {
        error: "test error".to_string(),
    };
    let version_mismatch: RpcResponse<String> = RpcResponse::VersionMismatch {
        daemon_build_timestamp: 9876543210,
    };

    // Test that responses can be serialized
    assert!(serde_json::to_string(&success_response).is_ok());
    assert!(serde_json::to_string(&error_response).is_ok());
    assert!(serde_json::to_string(&version_mismatch).is_ok());
}

#[test]
fn test_daemon_status_serialization() {
    let status_ready = DaemonStatus::Ready;
    let status_busy = DaemonStatus::Busy("Processing".to_string());
    let status_error = DaemonStatus::Error("Failed".to_string());

    // Test serialization
    assert!(serde_json::to_string(&status_ready).is_ok());
    assert!(serde_json::to_string(&status_busy).is_ok());
    assert!(serde_json::to_string(&status_error).is_ok());
}

#[test]
fn test_server_handle() {
    use tokio::sync::{broadcast, mpsc};

    let (request_tx, _request_rx) = mpsc::channel::<RequestEnvelope<TestMethod>>(32);
    let (status_tx, _status_rx) = broadcast::channel(32);

    let server_handle = ServerHandle {
        request_tx,
        status_tx,
    };

    // Test that we can subscribe to status updates
    let status_receiver = server_handle.subscribe_status();

    // This should work without panicking
    drop(status_receiver);
}
