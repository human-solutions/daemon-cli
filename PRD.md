# Product Requirements Document: daemon-rpc

## Product Vision

A generic, reusable daemon-client framework for Rust applications that provides streaming command execution through a transparent stdin→stdout pipeline. The framework handles all daemon lifecycle management, version synchronization, and process coordination automatically.

## Product Goals

1. **Zero-Configuration Daemon Management**: Automatic daemon spawning, version checking, and lifecycle management
2. **Universal I/O Interface**: Standard stdin/stdout streaming works with pipes, scripts, and any language
3. **Single-Task Streaming Model**: One command at a time with real-time output streaming and cancellation support
4. **Transparent Operation**: CLI acts as pure pipe (stdin → daemon → stdout), errors only to stderr

## Success Metrics

1. **Performance**:
   - Warm request latency < 50ms (command to first output)
   - Cold start < 500ms (daemon spawn to first output)

2. **Simplicity**:
   - Zero configuration files or setup steps required
   - Works out-of-the-box with pipes and shell scripts

3. **Reliability**:
   - Client and daemon version mismatches auto-resolved in < 1s
   - Clean cancellation on Ctrl+C with no hung processes

## Core Architecture

### Single-Task Processing Model

- **One command at a time**: Daemon handles exactly one CLI connection
- **Additional connections fail immediately**: Ensures predictable resource usage and simple state management
- **Command isolation**: Each command is independent with full cleanup between executions

### Automatic Version Synchronization

- **Build timestamp comparison**: Client and daemon exchange build timestamps on connect
- **Auto-restart on mismatch**: Daemon automatically replaced with newer version
- **Zero user intervention**: Version sync happens transparently

### Streaming with Cancellation

- **Real-time output**: Daemon streams output chunks as they're produced
- **Ctrl+C support**: User can cancel long-running commands
- **Graceful shutdown**: Handler receives cancellation signal for cleanup
- **Force termination**: Timeout enforces maximum shutdown time

### Silent Operation

- **No status messages**: CLI outputs only daemon content to stdout
- **Error isolation**: Framework errors written to stderr only
- **Transparent pipeline**: `echo "cmd" | cli` behaves like native commands

## API Design

### Handler Interface

Applications implement a single trait to define command behavior:

```rust
trait CommandHandler {
    async fn handle(
        &self,
        command: &str,           // Full stdin as string
        output: impl AsyncWrite, // Stream output here
        cancel: CancellationToken // For Ctrl+C handling
    ) -> Result<()>;
}
```

**Handler Responsibilities:**
- Parse the command string (any format, handler decides)
- Stream output bytes via the `AsyncWrite` interface
- Respond to cancellation token by stopping gracefully
- Return `Result` for error handling

**Framework Responsibilities:**
- Read CLI stdin and deliver as command string
- Manage socket communication and output streaming
- Handle Ctrl+C and propagate cancellation
- Perform version checking and daemon spawning
- Write framework errors to stderr

### Usage Examples

**Simple command:**
```bash
echo "process /path/to/file" | my-cli
```

**Piping data:**
```bash
cat large-file.txt | my-cli compress
```

**Scripting:**
```bash
for file in *.txt; do
    echo "analyze $file" | my-cli
done
```

**From other languages (Python):**
```python
import subprocess
result = subprocess.run(['my-cli'], input='process data', text=True, capture_output=True)
print(result.stdout)
```

## Protocol Overview

### Message Types

The socket protocol uses four message types:

1. **VersionCheck** - Handshake with build timestamps
2. **Command(String)** - CLI sends full stdin to daemon (once)
3. **OutputChunk(Bytes)** - Daemon streams output to CLI (multiple)
4. **CommandError(String)** - Daemon reports handler error before closing

Connection close signals completion (no explicit message needed).

### Communication Flow

**Normal execution:**
1. CLI connects to socket (spawns daemon if needed)
2. Version handshake (restart daemon if mismatch)
3. CLI sends `Command` with stdin content
4. Daemon sends `OutputChunk` messages as output is produced
5. Daemon closes connection on success, or sends `CommandError` then closes on failure
6. CLI detects EOF and exits

**Cancellation (Ctrl+C):**
1. CLI closes connection immediately
2. Daemon detects broken connection
3. Daemon signals `CancellationToken` to handler
4. Handler stops gracefully (or is force-terminated after timeout)
5. Daemon ready for next connection

**Version mismatch:**
1. Handshake reveals timestamp difference
2. CLI terminates old daemon and spawns new one
3. CLI retries connection with new daemon

## Constraints & Limitations

### Design Constraints

- **Non-interactive**: Entire command must be provided via stdin (no interactive prompts)
- **Platform**: Unix-like systems only (Linux, macOS) via Unix domain sockets
- **Single client**: Only one connection at a time per daemon instance
- **No structured output**: Framework doesn't enforce output format (handler decides)

### Acceptable Limitations

- **No progress reporting**: Unless handler explicitly emits it in output stream
- **No command queuing**: Second connection attempt fails, doesn't queue
- **No bi-directional interaction**: Command sent once, no follow-up requests

## Future Considerations

Not in current scope, but potential future enhancements:

- Multi-client support with request queuing
- Interactive command mode with prompt/response cycles
- Structured output format enforcement
- Windows support via named pipes
- Progress reporting protocol extensions
