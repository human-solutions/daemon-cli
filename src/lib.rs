//! # daemon-rpc
//!
//! A zero-configuration framework for building daemon-based Rust applications.
//!
//! ## Overview
//!
//! daemon-rpc provides a simple way to build applications that communicate
//! with background daemon processes. The framework handles all the complexity
//! of daemon spawning, IPC, and error handling automatically.
//!
//! ## Key Features
//!
//! - **Zero Configuration**: Single `connect()` method handles everything
//! - **Automatic Spawning**: Daemons start automatically when needed
//! - **Type-Safe RPC**: Compile-time checked method calls
//! - **Status Streaming**: Real-time progress updates
//! - **Task Cancellation**: Graceful + forceful task termination
//! - **Version Management**: Automatic daemon restart on version mismatch
//! - **Socket-Based Coordination**: Daemons manage their own lifecycle via socket monitoring
//!
//! ## Quick Start
//!
//! ### 1. Define RPC Methods
//!
//! ```rust
//! use daemon_rpc::prelude::*;
//!
//! #[derive(Serialize, Deserialize, Debug, Clone)]
//! enum FileOperation {
//!     ProcessFile { path: std::path::PathBuf },
//!     GetStatus,
//! }
//!
//! impl RpcMethod for FileOperation {
//!     type Response = String;
//! }
//! ```
//!
//! ### 2. Implement Daemon Handler
//!
//! ```rust
//! use daemon_rpc::prelude::*;
//!
//! // First define the types (from step 1)
//! #[derive(Serialize, Deserialize, Debug, Clone)]
//! enum FileOperation {
//!     ProcessFile { path: std::path::PathBuf },
//!     GetStatus,
//! }
//!
//! impl RpcMethod for FileOperation {
//!     type Response = String;
//! }
//!
//! #[derive(Clone)]
//! struct FileDaemon;
//!
//! #[async_trait]
//! impl RpcHandler<FileOperation> for FileDaemon {
//!     async fn handle(
//!         &mut self,
//!         method: FileOperation,
//!         cancel_token: CancellationToken,
//!         status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
//!     ) -> Result<String> {
//!         match method {
//!             FileOperation::ProcessFile { path } => {
//!                 // Send status updates during processing
//!                 let _ = status_tx.send(DaemonStatus::Busy("Processing...".to_string())).await;
//!                 
//!                 // Your processing logic here
//!                 Ok(format!("Processed: {:?}", path))
//!             }
//!             FileOperation::GetStatus => Ok("Ready".to_string()),
//!         }
//!     }
//! }
//!
//! // Verify the handler implementation compiles
//! let daemon = FileDaemon;
//! let method = FileOperation::GetStatus;
//!
//! // Verify types are correct
//! let _: &dyn RpcHandler<FileOperation> = &daemon;
//! ```
//!
//! ### 3. Create Daemon Binary
//!
//! ```rust
//! use daemon_rpc::prelude::*;
//!
//! // Use the FileDaemon from step 2
//! # #[derive(Clone)]
//! # struct FileDaemon;
//! #
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum FileOperation {
//! #     ProcessFile { path: std::path::PathBuf },
//! #     GetStatus,
//! # }
//! #
//! # impl RpcMethod for FileOperation {
//! #     type Response = String;
//! # }
//! #
//! # #[async_trait]
//! # impl RpcHandler<FileOperation> for FileDaemon {
//! #     async fn handle(
//! #         &mut self,
//! #         method: FileOperation,
//! #         _cancel_token: CancellationToken,
//! #         _status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
//! #     ) -> Result<String> {
//! #         Ok("Ready".to_string())
//! #     }
//! # }
//!
//! let daemon = FileDaemon;
//! let build_timestamp = 1234567890u64; // In real apps, from build.rs
//! let server = DaemonServer::new(1000, build_timestamp, daemon);
//!
//! // Verify server creation
//! assert_eq!(server.daemon_id, 1000);
//! assert_eq!(server.build_timestamp, build_timestamp);
//!
//! // Actual server usage (would block listening for connections):
//! # tokio_test::block_on(async {
//! #     // Would start the daemon server (skipped in doc test)
//! #     // server.spawn_with_socket().await?;
//! # });
//! ```
//!
//! ### 4. Use from Client
//!
//! ```rust
//! use daemon_rpc::prelude::*;
//! use std::path::PathBuf;
//!
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum FileOperation {
//! #     ProcessFile { path: PathBuf },
//! # }
//! #
//! # impl RpcMethod for FileOperation {
//! #     type Response = String;
//! # }
//!
//! // Use tokio_test for doc tests with async
//! tokio_test::block_on(async {
//!     // Note: This would normally connect to a real daemon
//!     // For doc test purposes, we'll just show the API structure
//!     let daemon_exe = PathBuf::from("./target/debug/examples/cli");
//!     
//!     // This demonstrates the zero-config API - normally works automatically
//!     // let mut client = DaemonClient::connect(1000, daemon_exe, 1234567890).await?;
//!     // let response = client.request(FileOperation::ProcessFile {
//!     //     path: "input.txt".into()
//!     // }).await?;
//!     
//!     // Result handling pattern:
//!     // match response {
//!     //     RpcResponse::Success { output } => println!("Processed: {}", output),
//!     //     RpcResponse::Error { error } => println!("Error: {}", error),
//!     //     _ => println!("Unexpected response"),
//!     // }
//! });
//! ```
//!
//! ## Architecture
//!
//! - **Client Process**: Your application using `DaemonClient`
//! - **Daemon Process**: Background service using `DaemonServer`
//! - **Unix Socket IPC**: Fast, secure local communication
//! - **Single-Task Processing**: One operation at a time for predictability
//! - **Status Broadcasting**: Real-time updates to all connected clients

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub mod client;
pub mod server;
pub mod transport;

pub use client::DaemonClient;
pub use server::DaemonServer;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

pub mod prelude {
    pub use crate::*;
    pub use anyhow::Result;
    pub use async_trait::async_trait;
    pub use serde::{Deserialize, Serialize};
    pub use tokio_util::sync::CancellationToken;
}

/// Core trait for defining RPC methods that can be sent to daemon processes.
///
/// Implement this trait for your method enums to define the operations
/// your daemon can perform.
///
/// # Example
///
/// ```rust
/// use daemon_rpc::prelude::*;
/// use std::path::PathBuf;
///
/// #[derive(Serialize, Deserialize, Debug, Clone)]
/// enum FileOperation {
///     ProcessFile { path: PathBuf },
///     GetStatus,
///     Cancel,
/// }
///
/// impl RpcMethod for FileOperation {
///     type Response = String;  // All operations return String responses
/// }
///
/// // Verify the trait is implemented correctly
/// let method = FileOperation::GetStatus;
/// let json = serde_json::to_string(&method).unwrap();
/// assert!(json.contains("GetStatus"));
/// ```
pub trait RpcMethod: Serialize + DeserializeOwned + Send + Sync + Clone {
    /// The response type returned by all methods in this RPC interface.
    /// Must be serializable and implement Debug for error reporting.
    type Response: Serialize + DeserializeOwned + Send + Sync + std::fmt::Debug;
}

/// RPC request message sent from client to daemon.
///
/// Contains the method to execute and the client's build timestamp
/// for version compatibility checking.
#[derive(Debug, Serialize, Deserialize)]
#[serde(bound = "M: Serialize + DeserializeOwned")]
pub struct RpcRequest<M: RpcMethod> {
    /// The RPC method to execute on the daemon
    pub method: M,
    /// Client's build timestamp for version checking
    pub client_build_timestamp: u64,
}

/// RPC response message sent from daemon to client.
///
/// Represents the result of executing an RPC method, including
/// success, error, and version mismatch scenarios.
#[derive(Debug, Serialize, Deserialize)]
pub enum RpcResponse<R> {
    /// Method executed successfully with the given output
    Success {
        /// The response data from the successful method execution
        output: R,
    },
    /// Method execution failed with an error message
    Error {
        /// Human-readable error description
        error: String,
    },
    /// Client and daemon have incompatible versions - daemon will be restarted
    VersionMismatch {
        /// The daemon's build timestamp that caused the mismatch
        daemon_build_timestamp: u64,
    },
}

/// Real-time status updates broadcast by daemon to all connected clients.
///
/// Provides visibility into daemon state and current operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DaemonStatus {
    /// Daemon is idle and ready to accept new requests
    Ready,
    /// Daemon is processing a request with progress information
    Busy(
        /// Human-readable description of current activity
        String,
    ),
    /// Daemon encountered an error and cannot process requests
    Error(
        /// Error message describing what went wrong
        String,
    ),
}

/// Handler trait for implementing daemon-side RPC method processing.
///
/// Implement this trait on your daemon struct to define how each
/// RPC method should be processed.
///
/// # Example
///
/// ```rust,no_run
/// use daemon_rpc::prelude::*;
/// use std::time::Duration;
///
/// #[derive(Clone)]
/// struct FileProcessor;
///
/// // Define your RPC methods first
/// #[derive(Serialize, Deserialize, Debug, Clone)]
/// enum FileOperation {
///     ProcessFile { path: std::path::PathBuf },
///     GetStatus,
/// }
///
/// impl RpcMethod for FileOperation {
///     type Response = String;
/// }
///
/// #[async_trait]
/// impl RpcHandler<FileOperation> for FileProcessor {
///     async fn handle(
///         &mut self,
///         method: FileOperation,
///         cancel_token: CancellationToken,
///         status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
///     ) -> Result<String> {
///         match method {
///             FileOperation::ProcessFile { path } => {
///                 // Send status updates during processing
///                 status_tx.send(DaemonStatus::Busy("Processing file...".to_string())).await.ok();
///                 
///                 // Check for cancellation during long operations
///                 if cancel_token.is_cancelled() {
///                     return Err(anyhow::anyhow!("Operation cancelled"));
///                 }
///                 
///                 // Simulate work
///                 tokio::time::sleep(Duration::from_millis(100)).await;
///                 
///                 Ok(format!("Processed file: {:?}", path))
///             }
///             FileOperation::GetStatus => {
///                 Ok("Daemon ready".to_string())
///             }
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait RpcHandler<M: RpcMethod>: Send + Sync {
    /// Process an RPC method call with cancellation and status reporting support.
    ///
    /// # Parameters
    ///
    /// * `method` - The RPC method to execute
    /// * `cancel_token` - Token to check for cancellation requests
    /// * `status_tx` - Channel to send status updates to clients
    ///
    /// # Returns
    ///
    /// The method response or an error if processing failed
    ///
    /// # Cancellation
    ///
    /// Long-running operations should periodically check `cancel_token.is_cancelled()`
    /// and return early with an appropriate error if cancellation is requested.
    ///
    /// # Status Updates
    ///
    /// Send progress updates via `status_tx.send(DaemonStatus::Busy("message"))`
    /// to keep clients informed of long-running operations.
    async fn handle(
        &mut self,
        method: M,
        cancel_token: CancellationToken,
        status_tx: mpsc::Sender<DaemonStatus>,
    ) -> Result<M::Response>;
}
