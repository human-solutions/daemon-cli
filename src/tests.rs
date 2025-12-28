use crate::transport::SocketMessage;
use crate::*;
use std::collections::HashMap;
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
        _ctx: CommandContext,
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
    let version_msg: SocketMessage<()> = SocketMessage::VersionCheck {
        build_timestamp: 1234567890,
    };
    let serialized = serde_json::to_string(&version_msg).unwrap();
    let deserialized: SocketMessage<()> = serde_json::from_str(&serialized).unwrap();
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
        theme: None,
    };
    let mut env_vars = HashMap::new();
    env_vars.insert("TEST_VAR".to_string(), "test_value".to_string());
    let context = CommandContext::with_env(terminal_info.clone(), env_vars);
    let command_msg: SocketMessage<()> = SocketMessage::Command {
        command: "test command".to_string(),
        context,
    };
    let serialized = serde_json::to_string(&command_msg).unwrap();
    let deserialized: SocketMessage<()> = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::Command { command, context } => {
            assert_eq!(command, "test command");
            assert_eq!(context.terminal_info.width, Some(80));
            assert_eq!(context.terminal_info.height, Some(24));
            assert!(context.terminal_info.is_tty);
            assert_eq!(context.terminal_info.color_support, ColorSupport::Truecolor);
            assert_eq!(
                context.env_vars.get("TEST_VAR"),
                Some(&"test_value".to_string())
            );
        }
        _ => panic!("Wrong message type"),
    }

    // Test OutputChunk message
    let chunk_msg: SocketMessage<()> = SocketMessage::OutputChunk(vec![1, 2, 3, 4, 5]);
    let serialized = serde_json::to_string(&chunk_msg).unwrap();
    let deserialized: SocketMessage<()> = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::OutputChunk(data) => {
            assert_eq!(data, vec![1, 2, 3, 4, 5]);
        }
        _ => panic!("Wrong message type"),
    }

    // Test CommandComplete message
    let complete_msg: SocketMessage<()> = SocketMessage::CommandComplete { exit_code: 0 };
    let serialized = serde_json::to_string(&complete_msg).unwrap();
    let deserialized: SocketMessage<()> = serde_json::from_str(&serialized).unwrap();
    match deserialized {
        SocketMessage::CommandComplete { exit_code } => {
            assert_eq!(exit_code, 0);
        }
        _ => panic!("Wrong message type"),
    }

    // Test CommandError message
    let error_msg: SocketMessage<()> = SocketMessage::CommandError("test error".to_string());
    let serialized = serde_json::to_string(&error_msg).unwrap();
    let deserialized: SocketMessage<()> = serde_json::from_str(&serialized).unwrap();
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
        theme: None,
    };
    let ctx = CommandContext::new(terminal_info);

    let result = handler.handle("test", ctx, &mut output, cancel).await;
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
            _ctx: CommandContext,
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
        theme: None,
    };
    let ctx = CommandContext::new(terminal_info);

    // Cancel immediately
    cancel.cancel();

    let result = handler.handle("test", ctx, &mut output, cancel).await;
    assert!(result.is_err());
    assert!(String::from_utf8(output).unwrap().contains("Cancelled"));
}

#[test]
fn test_env_var_filter_none() {
    let filter = EnvVarFilter::none();
    assert!(filter.filter_current_env().is_empty());
}

#[test]
fn test_env_var_filter_with_names() {
    let mock_env = [("TEST_VAR", "test_value"), ("OTHER_VAR", "other")];
    let filter = EnvVarFilter::with_names(["TEST_VAR"]);
    let filtered = filter.filter_from(mock_env);
    assert_eq!(filtered.get("TEST_VAR"), Some(&"test_value".to_string()));
    assert_eq!(filtered.len(), 1);
}

#[test]
fn test_env_var_filter_include() {
    let mock_env = [("VAR1", "value1"), ("VAR2", "value2"), ("VAR3", "value3")];
    let filter = EnvVarFilter::none().include("VAR1").include("VAR2");
    let filtered = filter.filter_from(mock_env);
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered.get("VAR1"), Some(&"value1".to_string()));
    assert_eq!(filtered.get("VAR2"), Some(&"value2".to_string()));
}

#[test]
fn test_env_var_filter_missing_var() {
    // Filter for a var that doesn't exist
    let filter = EnvVarFilter::with_names(["NONEXISTENT_VAR_12345"]);
    let filtered = filter.filter_current_env();
    assert!(filtered.is_empty());
}

// ============================================================================
// Custom Payload Tests
// ============================================================================

/// Test payload type for unit tests
#[derive(serde::Serialize, serde::Deserialize, Clone, Default, Debug, PartialEq)]
struct TestPayload {
    value: String,
    count: u32,
}

#[async_trait]
impl PayloadCollector for TestPayload {
    async fn collect() -> Self {
        Self {
            value: "collected".to_string(),
            count: 42,
        }
    }
}

#[tokio::test]
async fn test_payload_collector_custom_type() {
    let payload = TestPayload::collect().await;
    assert_eq!(payload.value, "collected");
    assert_eq!(payload.count, 42);
}

#[tokio::test]
async fn test_payload_collector_unit_type() {
    // Verify the default () implementation works
    let payload = <()>::collect().await;
    assert_eq!(payload, ());
}

#[test]
fn test_command_context_with_payload_serialization() {
    let terminal_info = TerminalInfo {
        width: Some(120),
        height: Some(40),
        is_tty: true,
        color_support: ColorSupport::Truecolor,
        theme: Some(Theme::Dark),
    };
    let payload = TestPayload {
        value: "test-value".to_string(),
        count: 99,
    };
    let ctx = CommandContext::with_payload(terminal_info.clone(), payload);

    // Serialize to JSON
    let json = serde_json::to_string(&ctx).unwrap();

    // Deserialize back
    let deserialized: CommandContext<TestPayload> = serde_json::from_str(&json).unwrap();

    // Verify all fields
    assert_eq!(deserialized.terminal_info.width, Some(120));
    assert_eq!(deserialized.terminal_info.height, Some(40));
    assert!(deserialized.terminal_info.is_tty);
    assert_eq!(deserialized.payload.value, "test-value");
    assert_eq!(deserialized.payload.count, 99);
}

#[test]
fn test_command_context_with_env_and_payload_serialization() {
    let terminal_info = TerminalInfo {
        width: None,
        height: None,
        is_tty: false,
        color_support: ColorSupport::None,
        theme: None,
    };
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_VAR".to_string(), "my_value".to_string());
    let payload = TestPayload {
        value: "with-env".to_string(),
        count: 7,
    };

    let ctx = CommandContext::with_env_and_payload(terminal_info, env_vars, payload);

    // Serialize and deserialize
    let json = serde_json::to_string(&ctx).unwrap();
    let deserialized: CommandContext<TestPayload> = serde_json::from_str(&json).unwrap();

    // Verify payload survived
    assert_eq!(deserialized.payload.value, "with-env");
    assert_eq!(deserialized.payload.count, 7);
    // Verify env vars survived
    assert_eq!(
        deserialized.env_vars.get("MY_VAR"),
        Some(&"my_value".to_string())
    );
}

#[test]
fn test_socket_message_with_custom_payload() {
    let terminal_info = TerminalInfo {
        width: Some(80),
        height: Some(24),
        is_tty: false,
        color_support: ColorSupport::Basic16,
        theme: None,
    };
    let payload = TestPayload {
        value: "socket-test".to_string(),
        count: 123,
    };
    let context = CommandContext::with_payload(terminal_info, payload);

    let msg: SocketMessage<TestPayload> = SocketMessage::Command {
        command: "my-command".to_string(),
        context,
    };

    // Serialize and deserialize
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: SocketMessage<TestPayload> = serde_json::from_str(&json).unwrap();

    match deserialized {
        SocketMessage::Command { command, context } => {
            assert_eq!(command, "my-command");
            assert_eq!(context.payload.value, "socket-test");
            assert_eq!(context.payload.count, 123);
        }
        _ => panic!("Expected Command message"),
    }
}

#[test]
fn test_handler_with_custom_payload_compiles() {
    // Test that a handler with custom payload type compiles correctly
    #[derive(Clone)]
    struct PayloadTestHandler;

    #[async_trait]
    impl CommandHandler<TestPayload> for PayloadTestHandler {
        async fn handle(
            &self,
            _command: &str,
            ctx: CommandContext<TestPayload>,
            mut output: impl AsyncWrite + Send + Unpin,
            _cancel: CancellationToken,
        ) -> Result<i32> {
            // Access the payload
            let msg = format!("Payload: {} ({})\n", ctx.payload.value, ctx.payload.count);
            output.write_all(msg.as_bytes()).await?;
            Ok(0)
        }
    }

    // Just verify it compiles
    let _handler = PayloadTestHandler;
}

#[tokio::test]
async fn test_handler_receives_payload() {
    #[derive(Clone)]
    struct PayloadEchoHandler;

    #[async_trait]
    impl CommandHandler<TestPayload> for PayloadEchoHandler {
        async fn handle(
            &self,
            _command: &str,
            ctx: CommandContext<TestPayload>,
            mut output: impl AsyncWrite + Send + Unpin,
            _cancel: CancellationToken,
        ) -> Result<i32> {
            // Echo payload values to output
            output
                .write_all(format!("{}:{}", ctx.payload.value, ctx.payload.count).as_bytes())
                .await?;
            Ok(0)
        }
    }

    let handler = PayloadEchoHandler;
    let mut output = Vec::new();
    let cancel = CancellationToken::new();
    let terminal_info = TerminalInfo {
        width: None,
        height: None,
        is_tty: false,
        color_support: ColorSupport::None,
        theme: None,
    };
    let payload = TestPayload {
        value: "hello".to_string(),
        count: 42,
    };
    let ctx = CommandContext::with_payload(terminal_info, payload);

    let result = handler.handle("test", ctx, &mut output, cancel).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
    assert_eq!(String::from_utf8(output).unwrap(), "hello:42");
}
