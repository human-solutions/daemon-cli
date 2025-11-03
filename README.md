# daemon-cli

A streaming daemon-client framework for Rust with automatic lifecycle management and stdin/stdout streaming.

## Features

- Auto-spawning daemons with mtime-based version synchronization
- Streaming stdin/stdout interface
- Ctrl+C cancellation support
- Custom exit codes (0-255) for command results
- Low latency (< 50ms warm, < 500ms cold)

## Installation

```toml
[dependencies]
daemon-cli = "0.3.0"
```

## Usage

**Implement a handler:**

```rust
use daemon_cli::prelude::*;

struct MyHandler;

#[async_trait]
impl CommandHandler for MyHandler {
    async fn handle(
        &self,
        command: &str,
        mut output: impl AsyncWrite + Unpin + Send,
        cancel: CancellationToken,
    ) -> Result<i32> {
        output.write_all(format!("Received: {}\n", command).as_bytes()).await?;
        Ok(0)  // Return exit code (0 = success)
    }
}
```

**Run daemon:**

```rust
let root_path = "/path/to/project";
// Automatically detects daemon name and binary mtime
let (server, _handle) = DaemonServer::new(root_path, MyHandler);
server.run().await?;
// Optionally use handle.shutdown() to stop the server gracefully
```

**Run client:**

```rust
let root_path = "/path/to/project";
// Automatically detects daemon name, executable path, and binary mtime
let mut client = DaemonClient::connect(root_path).await?;
let exit_code = client.execute_command(command).await?;
std::process::exit(exit_code);  // Exit with the command's exit code
```

**Use it:**

```bash
echo "hello" | my-cli
cat file.txt | my-cli process
```

## Exit Codes

Handlers return custom exit codes (0-255) to indicate command results:

```rust
async fn handle(...) -> Result<i32> {
    match command.trim() {
        "status" => {
            output.write_all(b"Ready\n").await?;
            Ok(0)  // Success
        }
        "unknown" => {
            output.write_all(b"Unknown command\n").await?;
            Ok(127)  // Command not found (shell convention)
        }
        _ => {
            // For unrecoverable errors, return Err
            // This becomes exit code 1 with error message to stderr
            Err(anyhow::anyhow!("Fatal error"))
        }
    }
}
```

- `Ok(0)` - Success
- `Ok(1-255)` - Application-specific error codes
- `Err(e)` - Unrecoverable error (becomes exit code 1 with error message)

## Logging

The library uses `tracing` for structured logging. Daemon implementations should initialize a tracing subscriber:

```rust
tracing_subscriber::fmt()
    .with_target(false)
    .compact()
    .init();
```

This provides automatic client context (`client{id=X}`) for all logs. Handlers can add custom spans for command-level or operation-level context. Client-side logs are suppressed but shown on errors.

See `examples/cli.rs` and `examples/concurrent.rs` for complete logging setup examples.

## Example

See `examples/cli.rs` for a complete working example:

```bash
cargo run --example cli -- daemon --daemon-name cli --daemon-path /tmp/test
echo "status" | cargo run --example cli
```

## Platform

Unix-like systems only (Linux, macOS). Uses Unix domain sockets.
