# daemon-cli

A streaming daemon-client framework for Rust with automatic lifecycle management and stdin/stdout streaming.

## Features

- Auto-spawning daemons with mtime-based version synchronization
- Streaming stdin/stdout interface
- Ctrl+C cancellation support
- Low latency (< 50ms warm, < 500ms cold)

## Installation

```toml
[dependencies]
daemon-cli = "0.2.0"
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
    ) -> Result<()> {
        output.write_all(format!("Received: {}\n", command).as_bytes()).await?;
        Ok(())
    }
}
```

**Run daemon:**

```rust
let daemon_name = "my-cli";
let daemon_path = "/path/to/project";
// Automatically detects binary mtime for version checking
let (server, _handle) = DaemonServer::new(daemon_name, daemon_path, MyHandler);
server.run().await?;
// Optionally use handle.shutdown() to stop the server gracefully
```

**Run client:**

```rust
let daemon_name = "my-cli";
let daemon_path = "/path/to/project";
// Automatically detects binary mtime for version checking
let mut client = DaemonClient::connect(daemon_name, daemon_path, daemon_exe).await?;
client.execute_command(command).await?;
```

**Use it:**

```bash
echo "hello" | my-cli
cat file.txt | my-cli process
```

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
