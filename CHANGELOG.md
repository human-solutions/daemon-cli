# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.0] - 2025-12-29

### Changed (BREAKING)

- Removed `EnvVarFilter` - use `PayloadCollector` trait instead for passing env vars
- `CommandContext<P>` is now generic with custom payload support via `PayloadCollector`
- Removed `DaemonClient::with_env_filter()` method

### Migration

```rust
// Before (0.8.0):
let client = DaemonClient::connect(path)
    .await?
    .with_env_filter(EnvVarFilter::with_names(["MY_VAR"]));

// After (0.9.0):
#[derive(Serialize, Deserialize, Clone, Default)]
struct MyPayload { my_var: Option<String> }

#[async_trait]
impl PayloadCollector for MyPayload {
    async fn collect() -> Self {
        Self { my_var: std::env::var("MY_VAR").ok() }
    }
}

let client = DaemonClient::<MyPayload>::connect(path).await?;
```

## [0.8.0] - 2025-12-09

### Changed (BREAKING)

- `CommandHandler::handle()` now takes `ctx: CommandContext` instead of `terminal_info: TerminalInfo`
- Access terminal info via `ctx.terminal_info`

### Added

- **Environment variable passing**: Pass env vars from client to daemon per-command
  - New `EnvVarFilter` for exact-match filtering of env var names
  - New `CommandContext` struct bundling terminal info + env vars
  - `DaemonClient::with_env_filter()` builder method
- **Terminal theme detection**: Detect dark/light mode via `terminal-colorsaurus`
  - New `Theme` enum (`Dark`, `Light`)
  - New `TerminalInfo.theme: Option<Theme>` field
  - Uses 50ms timeout, returns `None` if detection fails

## [0.7.0] - 2025-12-03

### Changed (BREAKING)

- `DaemonServer::new()` now requires `startup_reason: StartupReason` parameter
- `StartupReason` enum redesigned with four variants: `FirstStart`, `BinaryUpdated`, `Recovered`, `ForceRestarted`
- Client passes `--startup-reason` CLI arg when spawning daemon

## [0.6.0] - 2025-11-28

### Added

- Windows support (named pipes, process management)
- Git hooks via cargo-husky (pre-commit: fmt/clippy, pre-push: nextest)

## [0.5.0] - 2025-01-23

### Added

- **Force-stop functionality**: New `DaemonClient::force_stop()` method for programmatic daemon shutdown
  - Implements graceful shutdown with SIGTERM followed by SIGKILL fallback after 2 second timeout
  - Automatically cleans up PID and socket files
  - Returns clear error messages when daemon is not running
- **Automatic daemon recovery**: Opt-in auto-restart on connection failures
  - New `DaemonClient::restart()` method for manual daemon restarts
  - New `DaemonClient::with_auto_restart(bool)` method to enable automatic recovery from crashes
  - Detects fatal connection errors (broken pipe, connection reset, connection closed)
  - Single retry logic prevents infinite loops
- CLI example `--stop` flag: `cargo run --example cli -- --stop`
- Public exports of `pid_path()` and `socket_path()` helper functions
- `nix` crate dependency for portable Unix signal handling

### Example

```rust
use daemon_cli::prelude::*;

// Force-stop daemon
let client = DaemonClient::connect("/path/to/project").await?;
client.force_stop().await?;

// Enable automatic recovery from crashes
let mut client = DaemonClient::connect("/path/to/project")
    .await?
    .with_auto_restart(true);
client.execute_command("some command".to_string()).await?;
// If daemon crashes, it will automatically restart and retry once

// Manual restart
client.restart().await?;
```

## [0.4.0] - 2025-01-15

### Changed (BREAKING)

- `CommandHandler::handle()` now takes `terminal_info: TerminalInfo` parameter
- `SocketMessage::Command` changed from `Command(String)` to `Command { command: String, terminal_info: TerminalInfo }`

### Added

- Terminal info detection: width, height, TTY status, color support
- New types: `TerminalInfo`, `ColorSupport`

### Migration

Add `terminal_info` parameter to your handler (prefix with `_` if unused):
```rust
async fn handle(
    &self,
    command: &str,
    terminal_info: TerminalInfo,  // NEW
    output: impl AsyncWrite + Send + Unpin,
    cancel_token: CancellationToken,
) -> Result<i32>
```

## [0.3.0] - 2025-01-XX

### Changed (BREAKING)

- **CommandHandler trait**: Changed return type from `Result<()>` to `Result<i32>`
  - Handlers now return exit codes (0-255) to indicate command results
  - `Ok(0)` indicates success
  - `Ok(1-255)` indicates application-specific error codes
  - `Err(e)` indicates unrecoverable errors (becomes exit code 1 with error message to stderr)

- **DaemonClient::execute_command()**: Changed return type from `Result<()>` to `Result<i32>`
  - Returns the exit code from the handler
  - Client applications should use `std::process::exit(exit_code)` to propagate the exit code

- **SocketMessage enum**: Changed `CommandComplete` variant from unit to struct
  - Old: `CommandComplete`
  - New: `CommandComplete { exit_code: i32 }`

- **DaemonServer::new()**: Simplified signature with automatic detection
  - Old: `DaemonServer::new(daemon_name, daemon_path, build_timestamp, handler)`
  - New: `DaemonServer::new(root_path, handler)`
  - Daemon name and build timestamp are now automatically detected
  - Parameter `daemon_path` renamed to `root_path` for clarity

- **DaemonServer::new_with_limit()**: Function removed and replaced
  - Old function removed entirely
  - Use `DaemonServer::new()` for standard setup (default 100 connection limit)
  - Use `DaemonServer::new_with_name_and_timestamp()` for testing with explicit parameters
  - Parameter `daemon_path` renamed to `root_path` in the new function

- **DaemonClient::connect()**: Simplified signature with automatic detection
  - Old: `DaemonClient::connect(daemon_name, daemon_path, daemon_executable, build_timestamp)`
  - New: `DaemonClient::connect(root_path)`
  - All parameters except `root_path` are now automatically detected

- **DaemonClient::connect_with_name_and_timestamp()**: New explicit connection function
  - Provides explicit control when automatic detection is not suitable
  - Primarily for testing scenarios
  - Parameter `daemon_path` renamed to `root_path`

- **Struct field renaming**: `daemon_path` → `root_path`
  - Affects public fields on both `DaemonClient` and `DaemonServer`
  - Improves clarity about the parameter's purpose

### Migration Guide

**Handler implementations:**
```rust
// Before (v0.2.0)
async fn handle(...) -> Result<()> {
    output.write_all(b"Done\n").await?;
    Ok(())
}

// After (v0.3.0)
async fn handle(...) -> Result<i32> {
    output.write_all(b"Done\n").await?;
    Ok(0)  // Return exit code
}
```

**Client implementations:**
```rust
// Before (v0.2.0)
client.execute_command(command).await?;
Ok(())

// After (v0.3.0)
let exit_code = client.execute_command(command).await?;
std::process::exit(exit_code)
```

**Server setup:**
```rust
// Before (v0.2.0)
let (server, _handle) = DaemonServer::new(
    "my-cli",
    "/path/to/project",
    build_timestamp,
    handler
);

// After (v0.3.0)
let (server, _handle) = DaemonServer::new("/path/to/project", handler);
// daemon_name and build_timestamp are auto-detected
```

**Client connection:**
```rust
// Before (v0.2.0)
let client = DaemonClient::connect(
    "my-cli",
    "/path/to/project",
    daemon_exe,
    build_timestamp
).await?;

// After (v0.3.0)
let client = DaemonClient::connect("/path/to/project").await?;
// All parameters are auto-detected
```

**Field access:**
```rust
// Before (v0.2.0)
let path = client.daemon_path;

// After (v0.3.0)
let path = client.root_path;
```

## [0.2.0] - 2025-01-XX

### Added

- Concurrent client support with configurable connection limiting
- Automatic daemon restart on version mismatch (mtime-based)
- Zero-configuration daemon management
- Streaming stdin/stdout interface
- Ctrl+C cancellation support

