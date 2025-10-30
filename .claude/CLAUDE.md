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
cargo run --example cli -- daemon --daemon-id 1000

# Execute commands (auto-spawns daemon)
echo "status" | cargo run --example cli
echo "process file.txt" | cargo run --example cli
```

## Architecture

### Core Components

**Client (`src/client.rs`)**
- `DaemonClient` - Handles connection to daemon with automatic spawning
- Auto-detects running daemons via socket existence
- Performs version handshake using build timestamps
- Restarts daemon on version mismatch
- Uses PID files (`/tmp/daemon-cli-{id}.pid`) for process cleanup

**Server (`src/server.rs`)**
- `DaemonServer` - Background daemon that processes commands
- `DaemonServer::new()` returns `(DaemonServer, DaemonHandle)` - always includes shutdown capability
- `DaemonHandle::shutdown()` gracefully stops the server; drop handle to run indefinitely
- Single-task processing model (one command at a time)
- Streaming output via tokio duplex channel
- Cancellation via `CancellationToken` when connection closes

**Transport (`src/transport.rs`)**
- Unix domain sockets at `/tmp/daemon-cli-{daemon_id}.sock`
- Length-delimited framing using tokio-util codec
- Message types: `VersionCheck`, `Command`, `OutputChunk`, `CommandError`
- Socket permissions: 0600 (owner only)

### Key Design Patterns

**Version Synchronization Flow:**
1. Client tries to connect to existing socket
2. If socket exists, performs version handshake
3. On mismatch: cleans up socket + PID file, spawns new daemon
4. New daemon performs fresh handshake

**Command Execution Flow:**
1. Client sends `Command` message with stdin content
2. Server creates duplex channel for streaming output
3. Handler writes output incrementally to channel
4. Server sends `OutputChunk` messages to client as data arrives
5. Client streams chunks to stdout
6. On handler completion: server closes connection (success) or sends `CommandError`

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

## Platform Requirements

- Unix-like systems only (Linux, macOS)
- Uses Unix domain sockets (not portable to Windows)
- Edition: Rust 2024
