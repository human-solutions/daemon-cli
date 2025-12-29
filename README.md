# daemon-cli

A streaming daemon-client framework for Rust with automatic lifecycle management and stdin/stdout streaming.

## Features

- Auto-spawning daemons with mtime-based version synchronization
- Streaming stdin/stdout interface
- Ctrl+C cancellation support
- Custom exit codes (0-255) for command results
- Generic payload support for passing custom data from client to daemon
- Cross-platform (Linux, macOS, Windows)

## Installation

```toml
[dependencies]
daemon-cli = "0.9.0"
```

## Usage

**Implement a handler:**

```rust
use daemon_cli::prelude::*;
use tokio::io::{AsyncWrite, AsyncWriteExt};

#[derive(Clone)]
struct MyHandler;

#[async_trait]
impl CommandHandler for MyHandler {
    async fn handle(
        &self,
        command: &str,
        ctx: CommandContext,
        mut output: impl AsyncWrite + Unpin + Send,
        cancel: CancellationToken,
    ) -> Result<i32> {
        // Access terminal info via ctx.terminal_info
        output.write_all(format!("Received: {}\n", command).as_bytes()).await?;
        Ok(0)  // Return exit code (0 = success)
    }
}
```

**Run daemon:**

```rust
let root_path = "/path/to/project";
let (server, _handle) = DaemonServer::new(root_path, MyHandler, StartupReason::default());
server.run().await?;
```

**Run client:**

```rust
let root_path = "/path/to/project";
let mut client = DaemonClient::connect(root_path).await?;
let exit_code = client.execute_command(command).await?;
std::process::exit(exit_code);
```

## Custom Payload

Pass custom data from client to daemon using `PayloadCollector`:

```rust
use daemon_cli::prelude::*;

#[derive(Serialize, Deserialize, Clone, Default)]
struct MyPayload {
    cwd: String,
    user: Option<String>,
}

#[async_trait]
impl PayloadCollector for MyPayload {
    async fn collect() -> Self {
        Self {
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            user: std::env::var("USER").ok(),
        }
    }
}

// Handler receives payload in ctx.payload
#[async_trait]
impl CommandHandler<MyPayload> for MyHandler {
    async fn handle(
        &self,
        command: &str,
        ctx: CommandContext<MyPayload>,
        mut output: impl AsyncWrite + Unpin + Send,
        cancel: CancellationToken,
    ) -> Result<i32> {
        println!("CWD: {}", ctx.payload.cwd);
        Ok(0)
    }
}

// Client with payload
let client = DaemonClient::<MyPayload>::connect(root_path).await?;
```

## Exit Codes

Handlers return custom exit codes (0-255):

- `Ok(0)` - Success
- `Ok(1-255)` - Application-specific error codes
- `Err(e)` - Unrecoverable error (becomes exit code 1)

## Example

See `examples/cli.rs` for a complete working example:

```bash
cargo run --example cli -- daemon
echo "status" | cargo run --example cli
```

## Platform

Cross-platform: Linux, macOS, Windows. Uses Unix domain sockets on Unix and named pipes on Windows.
