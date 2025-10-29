# daemon-rpc

A streaming daemon-client framework for Rust with automatic lifecycle management and stdin/stdout streaming.

## Features

- Auto-spawning daemons with version synchronization
- Streaming stdin/stdout interface
- Ctrl+C cancellation support
- Low latency (< 50ms warm, < 500ms cold)

## Installation

```toml
[dependencies]
daemon-rpc = "0.2.0"
```

## Usage

**Implement a handler:**

```rust
use daemon_rpc::prelude::*;

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
let (server, _handle) = DaemonServer::new(daemon_id, build_timestamp, MyHandler);
server.run().await?;
// Optionally use handle.shutdown() to stop the server gracefully
```

**Run client:**

```rust
let mut client = DaemonClient::connect(daemon_id, daemon_exe, build_timestamp).await?;
client.execute_command(command).await?;
```

**Use it:**

```bash
echo "hello" | my-cli
cat file.txt | my-cli process
```

## Example

See `examples/cli.rs` for a complete working example:

```bash
cargo run --example cli -- daemon --daemon-id 1000
echo "status" | cargo run --example cli
```

## Platform

Unix-like systems only (Linux, macOS). Uses Unix domain sockets.
