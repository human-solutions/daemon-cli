# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2025-01-15

### Changed (BREAKING)

- `CommandHandler::handle()` now takes `terminal_info: TerminalInfo` parameter
- `SocketMessage::Command` changed from `Command(String)` to `Command { command: String, terminal_info: TerminalInfo }`

### Added

- Terminal info detection: width, height, TTY status, color support, theme (light/dark)
- New types: `TerminalInfo`, `ColorSupport`, `Theme`

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

