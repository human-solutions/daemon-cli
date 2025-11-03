# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`daemon-cli` is a Rust library for building streaming daemon-client applications with automatic lifecycle management. It enables CLI tools to communicate with long-running background processes via stdin/stdout streaming over Unix domain sockets.

## Essential Commands

### Build and Test
```bash
cargo build                    # Build the library
cargo test                     # Run all tests (unit + integration)
cargo test --test integration_tests  # Run integration tests only
cargo check                    # Fast check without building
```

### Run Example
```bash
# Start daemon manually
cargo run --example cli -- daemon --daemon-name cli --daemon-path /tmp/test

# Execute commands (auto-spawns daemon)
echo "status" | cargo run --example cli
echo "process file.txt" | cargo run --example cli
```

## Architecture

### Core Components

**Client (`src/client.rs`)**
- `DaemonClient` - Handles connection to daemon with automatic spawning
- Auto-detects running daemons via socket existence
- Performs version handshake using binary modification time (mtime)
- Restarts daemon on version mismatch (client binary newer than daemon binary)
- Uses PID files (`/tmp/{short_id}-{daemon_name}.pid`) for process cleanup
- Requires `daemon_name` and `daemon_path` for identification

**Server (`src/server.rs`)**
- `DaemonServer` - Background daemon that processes commands
- `DaemonServer::new()` returns `(DaemonServer, DaemonHandle)` - default limit of 100 concurrent connections
- `DaemonServer::new_with_limit()` allows custom connection limit
- `DaemonHandle::shutdown()` gracefully stops the server; drop handle to run indefinitely
- Concurrent processing model (multiple clients and commands simultaneously)
- Each client connection handled in separate tokio task
- Connection limiting always enabled (default: 100, configurable)
- Streaming output via tokio duplex channel
- Cancellation via `CancellationToken` when connection closes
- Requires `daemon_name` and `daemon_path` parameters

**Transport (`src/transport.rs`)**
- Unix domain sockets at `/tmp/{short_id}-{daemon_name}.sock`
- `short_id` is a 4-character base62 hash of `daemon_path` for uniqueness
- Length-delimited framing using tokio-util codec
- Message types: `VersionCheck`, `Command`, `OutputChunk`, `CommandError`
- Socket permissions: 0600 (owner only)

### Key Design Patterns

**Version Synchronization Flow:**
1. Client automatically reads its own binary's modification time (mtime) at launch (via `DaemonClient::connect()`)
2. Client tries to connect to existing socket
3. If socket exists, performs version handshake by comparing mtimes
4. On mismatch (client mtime > daemon mtime): cleans up socket + PID file, spawns new daemon
5. New daemon automatically reads its own binary mtime at startup (via `DaemonServer::new()`) and performs fresh handshake
6. Version checking ensures daemon automatically restarts when binary is rebuilt - no user action required

**Command Execution Flow:**
1. Client sends `Command` message with stdin content
2. Server spawns task to handle connection (concurrent with other clients)
3. Server creates duplex channel for streaming output
4. Handler writes output incrementally to channel
5. Server sends `OutputChunk` messages to client as data arrives
6. Client streams chunks to stdout
7. On handler completion: server closes connection (success) or sends `CommandError`

Note: Multiple clients can execute commands concurrently. Each connection gets its own task and handler instance (via Clone).

**Cancellation Model:**
- Ctrl+C on client closes socket connection
- Server detects closed connection and triggers `CancellationToken`
- Handler should check `cancel_token.is_cancelled()` during long operations

### Handler Trait

The `CommandHandler` trait is the primary extension point:
```rust
#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn handle(
        &self,
        command: &str,           // Command string from stdin
        output: impl AsyncWrite,  // Stream output here incrementally
        cancel_token: CancellationToken,  // Check for cancellation
    ) -> Result<()>;
}
```

**Concurrency Considerations:**
- Handlers must implement `Clone + Send + Sync` for concurrent execution
- Each client connection receives a cloned handler instance
- If handlers need shared mutable state, use `Arc<Mutex<T>>` or similar
- Handlers may run concurrently - design for thread-safety
- For serial execution, implement queuing/routing within your handler

**Example with shared state:**
```rust
#[derive(Clone)]
struct MyHandler {
    shared_state: Arc<Mutex<HashMap<String, String>>>,
}
```

### Connection Limiting

Connection limiting is always enabled to prevent resource exhaustion. The default limit is **100 concurrent connections**.

**Using the default (100 connections):**
```rust
use daemon_cli::prelude::*;

let handler = MyHandler::new();
// Automatically detects binary mtime for version checking
let (server, _handle) = DaemonServer::new("my-cli", "/path/to/project", handler);
// Default: max 100 concurrent connections
```

**Customizing the limit:**
```rust
use daemon_cli::prelude::*;

let handler = MyHandler::new();
// Automatically detects binary mtime for version checking
let (server, _handle) = DaemonServer::new_with_limit(
    "my-cli",
    "/path/to/project",
    handler,
    10  // Max 10 concurrent clients
);
```

When the limit is reached, new connections wait for an available slot. This is implemented using a semaphore, so waiting clients will automatically proceed when other clients disconnect.

## Platform Requirements

- Unix-like systems only (Linux, macOS)
- Uses Unix domain sockets (not portable to Windows)
- Edition: Rust 2024

# Other memory

- Use the `gh` command to interact with GitHub
- Keep commit and PR messages brief
