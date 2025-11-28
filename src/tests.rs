use crate::transport::SocketMessage;
use crate::*;
use tokio::io::{AsyncWrite, AsyncWriteExt};

// Test handler for unit tests
#[derive(Clone)]
struct TestHandler {
    output_text: String,
}

impl TestHandler {
    fn new(output_text: String) -> Self {
        Self { output_text }
    }
}

#[async_trait]
impl CommandHandler for TestHandler {
    async fn handle(
        &self,
        _command: &str,
        _terminal_info: TerminalInfo,
        mut output: impl AsyncWrite + Send + Unpin,
        _cancel: CancellationToken,
    ) -> Result<i32> {
        output.write_all(self.output_text.as_bytes()).await?;
        Ok(0)
    }
}

#[test]
fn test_command_handler_trait_compiles() {
    // Verify the trait compiles correctly
    let _handler = TestHandler::new("test output".to_string());
    // Note: CommandHandler is not object-safe due to impl AsyncWrite parameter
    // This is expected and correct for our use case
}

#[test]
fn test_socket_message_serialization() {
    // Test VersionCheck message
    let version_msg = SocketMessage::VersionCheck {
        build_timestamp: 1234567890,
    };
    let serialized = serde_json::to_string(&version_msg).unwrap();
    let deserialized: SocketMessage = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::VersionCheck { build_timestamp } => {
            assert_eq!(build_timestamp, 1234567890);
        }
        _ => panic!("Wrong message type"),
    }

    // Test Command message
    let terminal_info = TerminalInfo {
        width: Some(80),
        height: Some(24),
        is_tty: true,
        color_support: ColorSupport::Truecolor,
    };
    let command_msg = SocketMessage::Command {
        command: "test command".to_string(),
        terminal_info: terminal_info.clone(),
    };
    let serialized = serde_json::to_string(&command_msg).unwrap();
    let deserialized: SocketMessage = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::Command {
            command,
            terminal_info: ti,
        } => {
            assert_eq!(command, "test command");
            assert_eq!(ti.width, Some(80));
            assert_eq!(ti.height, Some(24));
            assert!(ti.is_tty);
            assert_eq!(ti.color_support, ColorSupport::Truecolor);
        }
        _ => panic!("Wrong message type"),
    }

    // Test OutputChunk message
    let chunk_msg = SocketMessage::OutputChunk(vec![1, 2, 3, 4, 5]);
    let serialized = serde_json::to_string(&chunk_msg).unwrap();
    let deserialized: SocketMessage = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::OutputChunk(data) => {
            assert_eq!(data, vec![1, 2, 3, 4, 5]);
        }
        _ => panic!("Wrong message type"),
    }

    // Test CommandComplete message
    let complete_msg = SocketMessage::CommandComplete { exit_code: 0 };
    let serialized = serde_json::to_string(&complete_msg).unwrap();
    let deserialized: SocketMessage = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::CommandComplete { exit_code } => {
            assert_eq!(exit_code, 0);
        }
        _ => panic!("Wrong message type"),
    }

    // Test CommandError message
    let error_msg = SocketMessage::CommandError("test error".to_string());
    let serialized = serde_json::to_string(&error_msg).unwrap();
    let deserialized: SocketMessage = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::CommandError(err) => {
            assert_eq!(err, "test error");
        }
        _ => panic!("Wrong message type"),
    }
}

#[tokio::test]
async fn test_handler_basic_output() {
    let handler = TestHandler::new("Hello, World!".to_string());
    let mut output = Vec::new();
    let cancel = CancellationToken::new();
    let terminal_info = TerminalInfo {
        width: Some(80),
        height: Some(24),
        is_tty: true,
        color_support: ColorSupport::Basic16,
    };

    let result = handler
        .handle("test", terminal_info, &mut output, cancel)
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0); // Success exit code
    assert_eq!(String::from_utf8(output).unwrap(), "Hello, World!");
}

#[tokio::test]
async fn test_handler_with_cancellation() {
    // Test that handler receives cancellation token
    #[derive(Clone)]
    struct CancellableHandler;

    #[async_trait]
    impl CommandHandler for CancellableHandler {
        async fn handle(
            &self,
            _command: &str,
            _terminal_info: TerminalInfo,
            mut output: impl AsyncWrite + Send + Unpin,
            cancel: CancellationToken,
        ) -> Result<i32> {
            // Simulate work with cancellation checking
            for i in 0..10 {
                if cancel.is_cancelled() {
                    output.write_all(b"Cancelled\n").await?;
                    return Err(anyhow::anyhow!("Cancelled"));
                }
                output.write_all(format!("Step {}\n", i).as_bytes()).await?;
            }
            Ok(0)
        }
    }

    let handler = CancellableHandler;
    let mut output = Vec::new();
    let cancel = CancellationToken::new();
    let terminal_info = TerminalInfo {
        width: None,
        height: None,
        is_tty: false,
        color_support: ColorSupport::None,
    };

    // Cancel immediately
    cancel.cancel();

    let result = handler
        .handle("test", terminal_info, &mut output, cancel)
        .await;
    assert!(result.is_err());
    assert!(String::from_utf8(output).unwrap().contains("Cancelled"));
}
