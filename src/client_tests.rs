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

    // Connection will be cleaned up automatically when client is dropped
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

    // Simulate daemon crash by killing the specific daemon process
    let pid_file = crate::transport::pid_path(daemon_id);
    if pid_file.exists()
        && let Ok(pid_str) = std::fs::read_to_string(&pid_file)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
    {
        let _ = tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .output()
            .await;
    }

    // Give some time for the process to die and socket cleanup
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

    // Connection will be cleaned up automatically when client is dropped
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
    use crate::transport::{pid_path, socket_path};

    // Kill daemon using precise PID if available
    let pid_file = pid_path(daemon_id);
    if pid_file.exists()
        && let Ok(pid_str) = std::fs::read_to_string(&pid_file)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
    {
        let _ = tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .output()
            .await;
    }
    // Remove PID file
    let _ = std::fs::remove_file(&pid_file);

    // Wait for process termination
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Remove socket file if it exists
    let socket_path = socket_path(daemon_id);
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    // Brief cleanup delay
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

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
    let mut client: super::DaemonClient<TestMethod> = super::DaemonClient::connect(
        daemon_id,
        daemon_executable,
        2000, // Newer timestamp
    )
    .await
    .unwrap();

    // Request should detect version mismatch, restart daemon, and succeed
    // The version mismatch detection message is printed to stdout during restart
    let response = client.request(TestMethod::GetStatus).await.unwrap();
    match response {
        RpcResponse::Success { .. } => {
            // Success indicates the daemon was restarted and request succeeded
        }
        _ => panic!(
            "Expected success after automatic daemon restart, got: {:?}",
            response
        ),
    }
}
