use crate::*;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
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
