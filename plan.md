# Implementation Plan: daemon-rpc

## **✅ Phase 1: In-Memory Core Logic - COMPLETED** 
*Build and test the business logic without IPC complexity*

### **✅ Step 1.1: Single-Task State Machine - COMPLETED**
**Deliverable:** Daemon rejects concurrent requests when busy
**Acceptance Criteria:**
- ✅ First request succeeds and daemon enters "Busy" state
- ✅ Second request immediately fails with "Busy" response
- ✅ After first request completes, daemon returns to "Ready" state
- ✅ Subsequent requests succeed again

**Test Strategy:**
```rust
#[tokio::test]
async fn test_single_task_enforcement() {
    let (daemon_tx, mut daemon_rx) = mpsc::channel(1);
    let (status_tx, _status_rx) = mpsc::channel(32);
    
    // Send first request - should succeed
    // Send second request immediately - should fail with Busy
    // Wait for first to complete - daemon should be Ready
    // Send third request - should succeed
}
```

### **✅ Step 1.2: Working Cancellation Support - COMPLETED**
**Deliverable:** Long-running tasks can be cancelled mid-execution
**Acceptance Criteria:**
- ✅ Task responds to cancellation token within 100ms
- ✅ Daemon returns to "Ready" state after cancellation
- ✅ Cleanup is performed (no resource leaks)

**Test Strategy:**
```rust
#[tokio::test]
async fn test_task_cancellation() {
    // Start 10-second task
    // Cancel after 1 second
    // Verify task stops within 100ms
    // Verify daemon is Ready again
}
```

### **✅ Step 1.3: Status Streaming - COMPLETED**
**Deliverable:** Real-time status updates flow from daemon to client
**Acceptance Criteria:**
- ✅ Status updates sent during task execution
- ✅ Multiple clients can receive status updates
- ✅ Status stream continues across multiple requests
- ✅ Client receives "Ready" after task completion

**Test Strategy:**
```rust
#[tokio::test]
async fn test_status_streaming() {
    // Start long task with progress updates
    // Collect all status messages
    // Verify sequence: Ready -> Busy(0%) -> Busy(50%) -> Ready
}
```

---

## **✅ Phase 2: IPC Transport Layer - COMPLETED**
*Replace in-memory channels with Unix sockets*

### **✅ Step 2.1: Unix Socket Message Framing - COMPLETED**
**Deliverable:** Reliable message boundaries over Unix sockets
**Acceptance Criteria:**
- ✅ Send/receive complete messages without truncation
- ✅ Handle multiple messages in single read
- ✅ Handle partial messages across reads
- ✅ Socket cleanup on process exit

**Test Strategy:**
```rust
#[tokio::test]
async fn test_socket_framing() {
    // Send 100 messages of varying sizes rapidly
    // Verify all 100 received intact
    // Test with malformed frames
}
```

### **✅ Step 2.2: Request/Response Serialization - COMPLETED**
**Deliverable:** RPC requests work over Unix sockets
**Acceptance Criteria:**
- ✅ `RpcRequest<T>` serializes/deserializes correctly
- ✅ `RpcResponse<T>` handles all variants (Success/Error/VersionMismatch)
- ✅ Error handling for malformed messages
- ✅ Performance: <1ms serialization for typical requests

**Test Strategy:**
```rust
#[tokio::test]
async fn test_end_to_end_rpc() {
    // Start daemon process
    // Connect client via Unix socket
    // Send ProcessFile request
    // Verify response received
}
```

---

## **✅ Phase 3: Process Management - COMPLETED**
*Automatic daemon spawning and lifecycle*

### **✅ Step 3.1: Daemon Process Spawning - COMPLETED**
**Deliverable:** `DaemonClient::connect()` spawns daemon if not running
**Acceptance Criteria:**
- ✅ Detect if daemon is already running (by socket file)
- ✅ Spawn new daemon process if needed
- ✅ Wait for daemon to be ready before returning client
- ✅ Multiple clients can connect to same daemon
- ✅ Handle daemon startup failures gracefully

**Test Strategy:**
```rust
#[tokio::test]
async fn test_daemon_auto_spawn() {
    // Ensure no daemon running
    // Call DaemonClient::connect()
    // Verify daemon process started
    // Verify client can send requests
}
```

### **✅ Step 3.2: Health Monitoring and Cleanup - COMPLETED**
**Deliverable:** Robust daemon lifecycle management
**Acceptance Criteria:**
- ✅ Detect daemon crashes and restart automatically
- ✅ Clean shutdown on client disconnect
- ✅ Idle timeout (daemon exits after N minutes of inactivity)
- ✅ Orphaned socket cleanup

**Test Strategy:**
```rust
#[tokio::test]  
async fn test_daemon_crash_recovery() {
    // Connect client
    // Kill daemon process
    // Send request - should auto-restart daemon
    // Verify request succeeds
}
```

---

## **✅ Phase 4: Version Management - COMPLETED**
*Build timestamp checking and automatic updates*

### **✅ Step 4.1: Build Timestamp Checking - COMPLETED**
**Deliverable:** Version mismatch detection
**Acceptance Criteria:**
- ✅ Client sends build timestamp with each request
- ✅ Daemon compares with its own build timestamp
- ✅ Version mismatch returns `VersionMismatch` response
- ✅ Build timestamp embedded correctly in binaries

**Test Strategy:**
```rust
#[tokio::test]
async fn test_version_mismatch() {
    // Start daemon with timestamp 1000
    // Send request with timestamp 2000
    // Verify VersionMismatch response
}
```

### **✅ Step 4.2: Automatic Daemon Restart - COMPLETED**
**Deliverable:** Seamless version updates
**Acceptance Criteria:**
- ✅ Version mismatch triggers daemon restart
- ✅ Client retries request after restart
- ✅ In-flight requests handled gracefully during restart
- ✅ Sub-500ms restart time (cold start requirement)

**Test Strategy:**
```rust
#[tokio::test]
async fn test_version_update() {
    // Start daemon with old version
    // Send request with new version
    // Verify daemon restarts automatically
    // Verify request succeeds with new daemon
}
```

---

## **Phase 5: Full Example**
*Complete CLI application demonstrating all framework features*

### **Step 5.1: Interactive CLI Application**
**Deliverable:** Production-ready example showing real-world usage patterns
**Acceptance Criteria:**
- ✅ **Daemon startup feedback**: CLI shows "busy" status during 2-second daemon initialization, then "ready"
- ✅ **Long-running task management**: CLI can launch background tasks via daemon
- ✅ **Real-time status display**: CLI continuously shows daemon status during task execution
- ✅ **Task cancellation**: ESC key cancels running tasks gracefully
- ✅ **Clean separation**: Ctrl-C exits CLI but leaves daemon running for other clients
- ✅ **Multiple client support**: Demonstrate multiple CLI instances connecting to same daemon

**Application Architecture:**
```
┌─────────────────┐    Unix Socket    ┌──────────────────┐
│   CLI Client    │ ◄──────────────► │   Daemon Server  │
│                 │                   │                  │
│ • Status Display│                   │ • Long Tasks     │
│ • ESC Handling  │                   │ • Status Updates │
│ • Ctrl-C Exit   │                   │ • Cancellation   │
└─────────────────┘                   └──────────────────┘
```

**Example Commands:**
```bash
# Start CLI (auto-spawns daemon)
cargo run --example file_processor

# CLI Interface:
> Starting daemon... (busy for 2 seconds)
> Daemon ready.
> 
> Commands:
>   process <file>  - Process a file (long-running task)
>   status         - Check daemon status  
>   quit           - Exit CLI (daemon stays running)
>
> process large_file.txt
> Processing large_file.txt... (progress updates)
> [Press ESC to cancel]
> Task completed successfully.
```

**Test Strategy:**
```rust
#[tokio::test]
async fn test_full_cli_workflow() {
    // Start daemon with 2-second startup delay
    // Verify status progression: busy -> ready
    // Launch long-running task
    // Verify status updates during execution
    // Test cancellation via signal
    // Verify clean shutdown behavior
}
```

---

## **Success Criteria for Each Phase:**

**✅ Phase 1:** All core business logic tests pass with in-memory implementation *(COMPLETED)*
**✅ Phase 2:** Same tests pass but using Unix socket transport *(COMPLETED)*
**✅ Phase 3:** Full integration tests with automatic process spawning *(COMPLETED)*
**✅ Phase 4:** Version management integration tests pass *(COMPLETED)*
**Phase 5:** Full CLI example with interactive features and real-world usage patterns

## **Key Implementation Notes:**

1. **Start Simple:** Phase 1 uses `tokio::sync::mpsc` channels - no processes yet
2. **Incremental:** Each phase builds on the previous, keeping all tests passing
3. **Testable:** Every step has concrete tests that verify the feature works
4. **Independent:** Steps can be implemented in any order within a phase
5. **Performance-Aware:** Sub-100ms latency target validated in Phase 2

This plan ensures **continuous verifiable progress** rather than a big-bang implementation.