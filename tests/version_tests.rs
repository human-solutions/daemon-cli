use anyhow::Result;
use daemon_cli::{
    prelude::*,
    test_utils::{SocketClient, SocketMessage},
};
use std::time::Duration;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    spawn,
    time::sleep,
};

// Simple test handler
#[derive(Clone)]
struct SimpleHandler;

#[async_trait]
impl CommandHandler for SimpleHandler {
    async fn handle(
        &self,
        command: &str,
        _terminal_info: TerminalInfo,
        mut output: impl AsyncWrite + Send + Unpin,
        _cancel: CancellationToken,
    ) -> Result<i32> {
        output.write_all(command.as_bytes()).await?;
        Ok(0)
    }
}

#[tokio::test]
async fn test_version_handshake_success() -> Result<()> {
    let daemon_name = "test-6001";
    let root_path = "/tmp/test-6001";
    let build_timestamp = 1111111111;
    let handler = SimpleHandler;

    // Start server with specific build timestamp
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    // Connect with matching build timestamp
    let mut client = SocketClient::connect(daemon_name, root_path).await?;

    // Send version check
    client
        .send_message(&SocketMessage::VersionCheck { build_timestamp })
        .await?;

    // Receive response
    let response = client.receive_message::<SocketMessage>().await?;

    match response {
        Some(SocketMessage::VersionCheck {
            build_timestamp: daemon_ts,
        }) => {
            assert_eq!(daemon_ts, build_timestamp);
        }
        _ => panic!("Expected VersionCheck response"),
    }

    Ok(())
}

#[tokio::test]
async fn test_version_mismatch_detection() -> Result<()> {
    let daemon_name = "test-6002";
    let root_path = "/tmp/test-6002";
    let daemon_build_timestamp = 2222222222;
    let client_build_timestamp = 3333333333;
    let handler = SimpleHandler;

    // Start server with one timestamp
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        daemon_build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    // Connect with different build timestamp
    let mut client = SocketClient::connect(daemon_name, root_path).await?;

    // Send version check with mismatched timestamp
    client
        .send_message(&SocketMessage::VersionCheck {
            build_timestamp: client_build_timestamp,
        })
        .await?;

    // Receive response
    let response = client.receive_message::<SocketMessage>().await?;

    match response {
        Some(SocketMessage::VersionCheck {
            build_timestamp: daemon_ts,
        }) => {
            // Daemon should send its own timestamp back
            assert_eq!(daemon_ts, daemon_build_timestamp);
            assert_ne!(daemon_ts, client_build_timestamp);
        }
        _ => panic!("Expected VersionCheck response"),
    }

    // After mismatch, the connection should still be open for client to restart
    // In the real client implementation, it would restart the daemon here

    Ok(())
}

#[tokio::test]
async fn test_multiple_version_handshakes() -> Result<()> {
    let daemon_name = "test-6003";
    let root_path = "/tmp/test-6003";
    let build_timestamp = 4444444444;
    let handler = SimpleHandler;

    // Start server
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    // Connect and perform handshake multiple times
    for _ in 0..3 {
        let mut client = SocketClient::connect(daemon_name, root_path).await?;

        // Perform handshake
        client
            .send_message(&SocketMessage::VersionCheck { build_timestamp })
            .await?;

        let response = client.receive_message::<SocketMessage>().await?;

        match response {
            Some(SocketMessage::VersionCheck {
                build_timestamp: daemon_ts,
            }) => {
                assert_eq!(daemon_ts, build_timestamp);
            }
            _ => panic!("Expected VersionCheck response"),
        }

        // Close connection
        drop(client);

        sleep(Duration::from_millis(50)).await;
    }

    Ok(())
}

#[tokio::test]
async fn test_version_handshake_before_command() -> Result<()> {
    let daemon_name = "test-6004";
    let root_path = "/tmp/test-6004";
    let build_timestamp = 5555555555;
    let handler = SimpleHandler;

    // Start server
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    let mut client = SocketClient::connect(daemon_name, root_path).await?;

    // First, perform version handshake
    client
        .send_message(&SocketMessage::VersionCheck { build_timestamp })
        .await?;

    let handshake_response = client.receive_message::<SocketMessage>().await?;
    assert!(matches!(
        handshake_response,
        Some(SocketMessage::VersionCheck { .. })
    ));

    // Then, send a command
    let terminal_info = TerminalInfo {
        width: Some(80),
        height: Some(24),
        is_tty: true,
        color_support: ColorSupport::Basic16,
    };
    client
        .send_message(&SocketMessage::Command {
            command: "test command".to_string(),
            terminal_info,
        })
        .await?;

    // Should receive output chunks
    let output_response = client.receive_message::<SocketMessage>().await?;
    assert!(matches!(
        output_response,
        Some(SocketMessage::OutputChunk(_))
    ));

    Ok(())
}

#[tokio::test]
async fn test_command_without_handshake_fails() -> Result<()> {
    let daemon_name = "test-6005";
    let root_path = "/tmp/test-6005";
    let build_timestamp = 6666666666;
    let handler = SimpleHandler;

    // Start server
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    let mut client = SocketClient::connect(daemon_name, root_path).await?;

    // Try to send command without handshake
    // The server expects VersionCheck first, so it should close the connection
    let terminal_info = TerminalInfo {
        width: None,
        height: None,
        is_tty: false,
        color_support: ColorSupport::None,
    };
    client
        .send_message(&SocketMessage::Command {
            command: "test".to_string(),
            terminal_info,
        })
        .await?;

    // Connection should close or we get no response
    let response = client.receive_message::<SocketMessage>().await?;

    // Should either get None (connection closed) or the server ignores it
    // Based on our implementation, server expects VersionCheck first
    assert!(response.is_none());

    Ok(())
}

#[tokio::test]
async fn test_concurrent_version_handshakes() -> Result<()> {
    let daemon_name = "test-6006";
    let root_path = "/tmp/test-6006";
    let build_timestamp = 7777777777;
    let handler = SimpleHandler;

    // Start server
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    // Try to connect multiple clients concurrently
    let mut handles = vec![];

    for i in 0..3 {
        let daemon_name_clone = daemon_name.to_string();
        let root_path_clone = root_path.to_string();
        let handle = spawn(async move {
            sleep(Duration::from_millis(i * 20)).await; // Stagger connections

            let mut client = SocketClient::connect(&daemon_name_clone, &root_path_clone).await?;

            client
                .send_message(&SocketMessage::VersionCheck { build_timestamp })
                .await?;

            let response = client.receive_message::<SocketMessage>().await?;

            match response {
                Some(SocketMessage::VersionCheck {
                    build_timestamp: daemon_ts,
                }) => {
                    assert_eq!(daemon_ts, build_timestamp);
                    Ok::<_, anyhow::Error>(())
                }
                _ => panic!("Expected VersionCheck response"),
            }
        });

        handles.push(handle);
    }

    // Wait for all handshakes to complete
    for handle in handles {
        handle.await??;
    }

    Ok(())
}

// ============================================================================
// ADDITIONAL TESTS - Coverage gaps
// ============================================================================

#[tokio::test]
async fn test_version_mismatch_triggers_client_action() -> Result<()> {
    // This test verifies that version mismatch is properly detected and reported
    // The actual restart logic is in the client, but we verify the server response
    let daemon_name = "test-6007";
    let root_path = "/tmp/test-6007";
    let daemon_timestamp = 1000000000; // Old daemon version
    let client_timestamp = 2000000000; // New client version
    let handler = SimpleHandler;

    // Start server with old timestamp
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        daemon_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    // Connect with newer client timestamp
    let mut client = SocketClient::connect(daemon_name, root_path).await?;

    // Send version check with newer timestamp
    client
        .send_message(&SocketMessage::VersionCheck {
            build_timestamp: client_timestamp,
        })
        .await?;

    // Server should respond with its own (older) timestamp
    let response = client.receive_message::<SocketMessage>().await?;

    match response {
        Some(SocketMessage::VersionCheck {
            build_timestamp: daemon_ts,
        }) => {
            // Verify server reported its actual timestamp
            assert_eq!(
                daemon_ts, daemon_timestamp,
                "Server should report its own timestamp"
            );
            // Verify timestamps don't match (client would trigger restart)
            assert_ne!(daemon_ts, client_timestamp, "Timestamps should not match");
        }
        _ => panic!("Expected VersionCheck response"),
    }

    Ok(())
}

#[tokio::test]
async fn test_multiple_commands_same_connection() -> Result<()> {
    let daemon_name = "test-6008";
    let root_path = "/tmp/test-6008";
    let build_timestamp = 8888888888;
    let handler = SimpleHandler;

    // Start server
    let (server, _handle) = DaemonServer::new_with_name_and_timestamp(
        daemon_name,
        root_path,
        build_timestamp,
        handler,
        StartupReason::FirstStart,
        100,
    );
    let _server_handle = spawn(async move {
        server.run().await.ok();
    });

    sleep(Duration::from_millis(100)).await;

    let mut client = SocketClient::connect(daemon_name, root_path).await?;

    // First, perform version handshake
    client
        .send_message(&SocketMessage::VersionCheck { build_timestamp })
        .await?;

    let handshake_response = client.receive_message::<SocketMessage>().await?;
    assert!(matches!(
        handshake_response,
        Some(SocketMessage::VersionCheck { .. })
    ));

    // Send first command
    let terminal_info = TerminalInfo {
        width: Some(80),
        height: Some(24),
        is_tty: true,
        color_support: ColorSupport::Basic16,
    };
    client
        .send_message(&SocketMessage::Command {
            command: "first command".to_string(),
            terminal_info: terminal_info.clone(),
        })
        .await?;

    // Receive first command output
    let output1 = client.receive_message::<SocketMessage>().await?;
    assert!(
        matches!(output1, Some(SocketMessage::OutputChunk(_))),
        "Should receive output for first command"
    );

    // Receive CommandComplete
    let complete1 = client.receive_message::<SocketMessage>().await?;
    assert!(
        matches!(
            complete1,
            Some(SocketMessage::CommandComplete { exit_code: 0 })
        ),
        "First command should complete successfully"
    );

    // Attempt to send a second command on the same connection
    // The server uses one-shot semantics: it closes after handling one command
    client
        .send_message(&SocketMessage::Command {
            command: "second command".to_string(),
            terminal_info: terminal_info.clone(),
        })
        .await?;

    // The connection should be closed by the server, so we expect EOF (None)
    let response = client.receive_message::<SocketMessage>().await?;
    assert!(
        response.is_none(),
        "Connection should be closed after first command (one-shot semantics), got: {:?}",
        response
    );

    Ok(())
}
