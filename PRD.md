# Product Requirements Document: daemon-rpc

## Executive Summary

The `daemon-rpc` crate will extract and generalize the daemon/client architecture from language-query into a reusable library for Rust applications that need persistent background services with robust IPC, version management, and status reporting.

## Current Architecture Analysis

### Key Components Identified

**Daemon Management:**
- Daemon identifier allowing unique identification and management
- Automatic daemon startup when not running
- Process lifecycle management with cleanup on exit
- Idle timeout (1 hour) with automatic shutdown
- Unix domain socket-based communication

**IPC Protocol:**
- Length-prefixed JSON messages (big-endian u32 + payload)
- Request/response pattern with unique UUIDs
- Method-based routing system
- Synchronous request-response communication

**Version Control:**
- Build timestamp-based version checking
- Automatic daemon restart on version mismatch
- Graceful handling of incompatible client/daemon versions

**Status & Progress Tracking:**
- LSP progress notification integration
- Long-running task monitoring (indexing, etc.)
- Real-time status reporting
- Bi-directional communication for status updates

## Product Goals

### Primary Objectives
1. **Generic Daemon Framework**: Provide reusable daemon/client architecture for any Rust application
2. **Long-Running Task Support**: Handle tasks with progress updates and cancellation
3. **Bi-Directional Communication**: Support status updates from daemon to client
4. **Version Management**: Automatic detection and handling of version mismatches
5. **Robust Lifecycle Management**: Automatic startup, health checks, and cleanup

### Success Metrics
- Zero-configuration daemon management
- Sub-100ms warm request latency
- Automatic version compatibility enforcement
- Progress tracking for long-running operations
- Task cancellation/interruption support

## Core Features

### 1. Daemon Management
```rust
pub struct DaemonManager<T> {
    workspace: PathBuf,
    daemon_factory: Box<dyn DaemonFactory<T>>,
    version_strategy: VersionStrategy,
}

pub trait DaemonFactory<T> {
    async fn create_daemon(&self, workspace: &Path) -> Result<T>;
    fn daemon_executable(&self) -> PathBuf;
}
```

**Features:**
- Workspace isolation with configurable hashing
- Automatic daemon spawning and lifecycle management
- Health checks and automatic recovery
- Configurable idle timeouts
- Clean shutdown handling with signal propagation

### 2. Generic IPC Protocol
```rust
pub trait RpcMethod: Serialize + DeserializeOwned + Send + Sync {
    type Response: Serialize + DeserializeOwned + Send + Sync;
}

pub struct RpcRequest<M: RpcMethod> {
    pub id: String,
    pub method: M,
    pub client_version: u64,
}

pub enum RpcResponse<R> {
    Success { output: R },
    Error { error: String },
    VersionMismatch { daemon_version: u64, message: String },
}
```

**Features:**
- Type-safe method definitions
- Automatic serialization/deserialization
- Built-in error handling
- Request correlation via UUIDs

### 3. Long-Running Task Support
```rust
pub struct TaskHandle<T> {
    id: TaskId,
    status_receiver: mpsc::Receiver<TaskStatus<T>>,
    cancel_sender: mpsc::Sender<()>,
}

pub enum TaskStatus<T> {
    Started,
    Progress { message: String, percentage: Option<f64> },
    Completed(T),
    Failed(String),
    Cancelled,
}

pub trait LongRunningTask: Send + Sync {
    type Output: Serialize + DeserializeOwned;
    type Progress: Serialize + DeserializeOwned;

    async fn execute(&mut self, progress_tx: mpsc::Sender<Self::Progress>) -> Result<Self::Output>;
    async fn cancel(&mut self) -> Result<()>;
}
```

**Features:**
- Progress reporting with custom progress types
- Task cancellation/interruption
- Status streaming to client
- Automatic task lifecycle management

### 4. Version Management
```rust
pub enum VersionStrategy {
    BuildTimestamp,
    SemanticVersion(semver::Version),
    Custom(Box<dyn VersionChecker>),
}

pub trait VersionChecker: Send + Sync {
    fn are_compatible(&self, client: u64, daemon: u64) -> bool;
    fn should_restart_daemon(&self, client: u64, daemon: u64) -> bool;
}
```

**Features:**
- Flexible version checking strategies
- Automatic daemon restart on incompatibility
- Backward compatibility support
- Custom version logic

### 5. Bi-Directional Communication
```rust
pub struct BidirectionalClient<M: RpcMethod> {
    request_sender: mpsc::Sender<RpcRequest<M>>,
    notification_receiver: mpsc::Receiver<ServerNotification>,
}

pub enum ServerNotification {
    StatusUpdate(String),
    TaskProgress { task_id: TaskId, status: String },
    DaemonShutdown,
}
```

**Features:**
- Server-initiated notifications
- Real-time status updates
- Event streaming from daemon
- Connection state management

## API Design

### Client Side
```rust
use daemon_rpc::prelude::*;

#[derive(Serialize, Deserialize)]
enum MyMethod {
    ProcessFile { path: PathBuf },
    GetStatus,
}

impl RpcMethod for MyMethod {
    type Response = String;
}

// Usage
let manager = DaemonManager::builder()
    .workspace(&workspace_path)
    .daemon_factory(MyDaemonFactory::new())
    .version_strategy(VersionStrategy::BuildTimestamp)
    .idle_timeout(Duration::from_secs(3600))
    .build();

let client = manager.connect().await?;
let response = client.request(MyMethod::ProcessFile {
    path: "src/main.rs".into()
}).await?;
```

### Daemon Side
```rust
use daemon_rpc::prelude::*;

struct MyDaemon;

#[async_trait]
impl RpcHandler<MyMethod> for MyDaemon {
    async fn handle(&mut self, method: MyMethod) -> Result<String> {
        match method {
            MyMethod::ProcessFile { path } => {
                // Long-running processing with progress
                self.process_file_with_progress(path).await
            },
            MyMethod::GetStatus => Ok("Ready".to_string()),
        }
    }
}

// Usage
let daemon = DaemonServer::builder()
    .handler(MyDaemon)
    .workspace(&workspace_path)
    .build();

daemon.run().await?;
```

## Implementation Plan

### Phase 1: Core Infrastructure
- Basic daemon/client architecture
- Unix socket communication
- Simple request/response protocol
- Workspace isolation

### Phase 2: Advanced Features
- Long-running task support
- Progress tracking
- Task cancellation
- Version management

### Phase 3: Enhanced Communication
- Bi-directional messaging
- Server notifications
- Connection state management
- Error recovery

### Phase 4: Production Readiness
- Comprehensive testing
- Documentation
- Performance optimization
- Error handling improvements

## Technical Considerations

### Dependencies
- `tokio` for async runtime
- `serde` for serialization
- `anyhow` for error handling
- `uuid` for request correlation
- `tracing` for observability
- `signal-hook-tokio` for signal handling

### Platform Support
- Primary: Linux and macOS (Unix sockets)
- Future: Windows (Named pipes)

### Performance Requirements
- Cold start: < 500ms
- Warm requests: < 50ms
- Memory overhead: < 10MB per daemon
- Support for 100+ concurrent requests

### Security Considerations
- Socket file permissions (0600)
- Workspace isolation
- Process credential validation
- No network exposure (local-only)

## Migration Path

The existing language-query codebase will be refactored to:
1. Extract common daemon/IPC code into the `daemon-rpc` crate
2. Implement language-specific methods using the generic framework
3. Maintain backward compatibility during transition
4. Provide migration guide for other projects

This PRD provides a comprehensive foundation for creating a robust, reusable daemon-rpc crate that addresses all the requirements you outlined while being generic enough for use in other projects.
