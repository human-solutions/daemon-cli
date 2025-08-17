use crate::*;
use std::path::PathBuf;

// Use the same types as the all_in_one example for consistency
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum TestMethod {
    ProcessFile {
        path: PathBuf,
        options: ProcessOptions,
    },
    GetStatus,
    GetUptime,
    LongTask {
        duration_seconds: u64,
        description: String,
    },
    QuickTask {
        message: String,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ProcessOptions {
    pub compress: bool,
    pub validate: bool,
    pub backup: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct TestResult {
    pub processed_bytes: u64,
    pub files_created: Vec<String>,
    pub duration_ms: u64,
    pub status: String,
}

impl RpcMethod for TestMethod {
    type Response = TestResult;
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
    let mut client = super::DaemonClient::connect(daemon_id, daemon_executable, 1234567890)
        .await
        .unwrap();

    // Verify we can make requests to the spawned daemon
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { output } => {
            assert!(
                output.status.contains("ready"),
                "Expected ready status, got: {}",
                output.status
            );
        }
        _ => panic!("Expected success response, got: {:?}", response),
    }

    // Test a more complex request
    let response = client
        .request(TestMethod::ProcessFile {
            path: "/tmp/auto-spawn-test.txt".into(),
            options: ProcessOptions {
                compress: false,
                validate: false,
                backup: false,
            },
        })
        .await
        .unwrap();

    match response {
        RpcResponse::Success { output } => {
            assert!(
                output.status.contains("processed"),
                "Expected processed status, got: {}",
                output.status
            );
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
    let mut client = super::DaemonClient::connect(daemon_id, daemon_executable, 1234567890)
        .await
        .unwrap();

    // Verify daemon is working
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    assert!(matches!(response, RpcResponse::Success { .. }));

    // Simulate daemon crash by killing the process
    if let Some(child) = &mut client.daemon_process {
        let _ = child.kill().await;
    }

    // Give some time for the process to die
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Create a new client - this will detect the crashed daemon and restart it
    let mut new_client = super::DaemonClient::connect(
        daemon_id,
        get_test_daemon_path(), // Use function call instead of moved variable
        1234567890,
    )
    .await
    .unwrap();

    // Verify daemon is working again after restart
    let response = new_client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { output } => {
            assert!(
                output.status.contains("ready"),
                "Expected ready status, got: {}",
                output.status
            );
        }
        _ => panic!(
            "Expected success response after restart, got: {:?}",
            response
        ),
    }

    // Clean up
    let _ = new_client.shutdown().await;
}

// Helper functions for tests

fn get_test_daemon_path() -> std::path::PathBuf {
    // First try to build the example to ensure it exists
    let _ = std::process::Command::new("cargo")
        .args(["build", "--example", "cli"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();

    // Get the path to the cli example binary
    let mut exe_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    exe_path.push("target");
    exe_path.push("debug");
    exe_path.push("examples");
    exe_path.push("cli");

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

// Phase 4 Tests - Version Management

#[tokio::test]
async fn test_version_mismatch_detection() {
    let daemon_executable = get_test_daemon_path();
    let daemon_id = 67890;

    // Ensure no daemon is running
    cleanup_daemon(daemon_id).await;

    // Spawn daemon with old build timestamp
    let _old_daemon = tokio::process::Command::new(&daemon_executable)
        .arg("daemon")
        .arg("--daemon-id")
        .arg(daemon_id.to_string())
        .arg("--build-timestamp")
        .arg("1000") // Old timestamp
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Wait for daemon to start
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Create client with newer build timestamp via socket
    let mut client: super::DaemonClient<TestMethod> = super::DaemonClient::connect_via_socket(
        daemon_id,
        daemon_executable,
        2000, // Newer timestamp
    )
    .await
    .unwrap();

    // Request should return version mismatch
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::VersionMismatch {
            daemon_build_timestamp,
        } => {
            assert_eq!(daemon_build_timestamp, 1000);
        }
        _ => panic!("Expected version mismatch response, got: {:?}", response),
    }
}

#[tokio::test]
async fn test_version_auto_restart() {
    let daemon_executable = get_test_daemon_path();
    let daemon_id = 78901;

    // Ensure no daemon is running
    cleanup_daemon(daemon_id).await;

    // First, start a daemon with old build timestamp by spawning the all_in_one with old timestamp
    let mut old_daemon = tokio::process::Command::new(&daemon_executable)
        .arg("daemon")
        .arg("--daemon-id")
        .arg(daemon_id.to_string())
        .arg("--build-timestamp")
        .arg("1000") // Old timestamp
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Wait for old daemon to start
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Connect client with newer build timestamp - should detect mismatch and restart
    let mut client = super::DaemonClient::connect(
        daemon_id,
        daemon_executable,
        2000, // Newer timestamp
    )
    .await
    .unwrap();

    // Make a request - this should trigger version mismatch detection and auto-restart
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { output } => {
            assert!(
                output.status.contains("ready"),
                "Expected ready status, got: {}",
                output.status
            );
        }
        RpcResponse::VersionMismatch { .. } => {
            // Version mismatch should trigger automatic restart
            // Make another request to verify restart worked
            let retry_response = client.request(TestMethod::GetStatus).await.unwrap();
            match retry_response {
                RpcResponse::Success { output } => {
                    assert!(
                        output.status.contains("ready"),
                        "Expected ready status, got: {}",
                        output.status
                    );
                }
                _ => panic!(
                    "Expected success after automatic restart, got: {:?}",
                    retry_response
                ),
            }
        }
        _ => panic!("Unexpected response type: {:?}", response),
    }

    // Clean up
    let _ = old_daemon.kill().await;
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn test_task_cancellation_via_message() {
    let daemon_executable = get_test_daemon_path();
    let daemon_id = 89012;

    // Ensure no daemon is running
    cleanup_daemon(daemon_id).await;

    // Connect and spawn daemon
    let mut client = super::DaemonClient::connect(daemon_id, daemon_executable, 1234567890)
        .await
        .unwrap();

    // Start a long-running task
    let start_time = std::time::Instant::now();
    let client_task = tokio::spawn(async move {
        client
            .request(TestMethod::LongTask {
                duration_seconds: 5,
                description: "Test cancellation task".to_string(),
            })
            .await
    });

    // Wait a bit for task to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Create another client to send cancel message
    let mut cancel_client: super::DaemonClient<TestMethod> =
        super::DaemonClient::connect_via_socket(daemon_id, get_test_daemon_path(), 1234567890)
            .await
            .unwrap();

    // Cancel the running task
    let cancelled = cancel_client.cancel_current_task().await.unwrap();
    assert!(cancelled, "Should have successfully cancelled the task");

    // Wait for the original request to complete
    let result = client_task.await.unwrap();
    let elapsed = std::time::Instant::now().duration_since(start_time);

    // Task should complete quickly due to cancellation
    assert!(
        elapsed < std::time::Duration::from_millis(2000),
        "Task should complete quickly due to cancellation, took: {:?}",
        elapsed
    );

    // The result should be an error due to cancellation
    match result.unwrap() {
        RpcResponse::Error { error } => {
            assert!(
                error.contains("cancelled"),
                "Expected cancellation error, got: {}",
                error
            );
        }
        RpcResponse::Success { .. } => {
            // Task completed before cancellation - this is also valid
            println!("Task completed before cancellation could take effect");
        }
        _ => panic!("Unexpected response type"),
    }

    // Clean up
    let _ = cancel_client.shutdown().await;
}
