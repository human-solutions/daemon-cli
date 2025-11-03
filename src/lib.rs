//! # daemon-cli
//!
//! A zero-configuration framework for building daemon-based Rust applications
//! with stdin/stdout streaming.
//!
//! ## Overview
//!
//! daemon-cli provides a simple way to build CLI applications that communicate
//! with background daemon processes through a transparent stdin→stdout pipeline.
//! The framework handles all daemon spawning, version synchronization, and
//! lifecycle management automatically.
//!
//! ## Key Features
//!
//! - **Zero Configuration**: Automatic daemon spawning and lifecycle management
//! - **Universal I/O**: Standard stdin/stdout streaming works with pipes and scripts
//! - **Transparent Operation**: CLI acts as pure pipe (stdin → daemon → stdout)
//! - **Task Cancellation**: Graceful cancellation via Ctrl+C
//! - **Version Management**: Automatic daemon restart on version mismatch
//! - **Concurrent Processing**: Multiple clients can execute commands simultaneously (default limit: 100)
//!
//! ## Quick Start
//!
//! ### 1. Implement Command Handler
//!
//! ```rust
//! use daemon_cli::prelude::*;
//! use tokio::io::{AsyncWrite, AsyncWriteExt};
//!
//! #[derive(Clone)]
//! struct MyHandler;
//!
//! #[async_trait]
//! impl CommandHandler for MyHandler {
//!     async fn handle(
//!         &self,
//!         command: &str,
//!         mut output: impl AsyncWrite + Send + Unpin,
//!         cancel_token: CancellationToken,
//!     ) -> Result<()> {
//!         // Parse and process the command
//!         output.write_all(b"Processing: ").await?;
//!         output.write_all(command.as_bytes()).await?;
//!         output.write_all(b"\n").await?;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ### 2. Create Daemon Binary
//!
//! ```rust,no_run
//! use daemon_cli::prelude::*;
//! # use tokio::io::{AsyncWrite, AsyncWriteExt};
//! #
//! # #[derive(Clone)]
//! # struct MyHandler;
//! #
//! # #[async_trait]
//! # impl CommandHandler for MyHandler {
//! #     async fn handle(
//! #         &self,
//! #         command: &str,
//! #         mut output: impl AsyncWrite + Send + Unpin,
//! #         _cancel_token: CancellationToken,
//! #     ) -> Result<()> {
//! #         output.write_all(command.as_bytes()).await?;
//! #         Ok(())
//! #     }
//! # }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let handler = MyHandler;
//!     let build_timestamp = 1234567890u64; // From build.rs
//!     let (server, _handle) = DaemonServer::new("my-cli", "/path/to/project", build_timestamp, handler);
//!     server.run().await?;
//!     Ok(())
//! }
//! ```
//!
//! ### 3. Use from CLI
//!
//! ```bash
//! # Simple command
//! echo "process file.txt" | my-cli
//!
//! # Piping data
//! cat large-file.txt | my-cli compress
//!
//! # From scripts
//! for file in *.txt; do
//!     echo "analyze $file" | my-cli
//! done
//! ```
//!

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::AsyncWrite;
use tokio_util::sync::CancellationToken;

mod client;
mod error_context;
mod server;
mod transport;

pub use client::DaemonClient;
pub use error_context::ErrorContextBuffer;
pub use server::{DaemonHandle, DaemonServer};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

/// Convenient re-exports for common daemon-cli types and traits.
///
/// Use `use daemon_cli::prelude::*;` to import all commonly needed items.
pub mod prelude {
    pub use crate::{CommandHandler, DaemonClient, DaemonHandle, DaemonServer, ErrorContextBuffer};
    pub use anyhow::Result;
    pub use async_trait::async_trait;
    pub use tokio_util::sync::CancellationToken;
}

// Internal testing utilities - not part of the public API
// These types are exposed for integration testing but are not part of the stable API
// and may change without warning. Do not use in production code.
#[doc(hidden)]
pub mod test_utils {
    pub use crate::transport::{SocketClient, SocketMessage};
}

/// Handler trait for processing commands received via stdin.
///
/// Implement this trait on your daemon struct to define how commands
/// are processed and output is streamed back to the client.
///
/// # Concurrency
///
/// Handlers may be invoked concurrently for multiple client connections.
/// The daemon clones your handler (via `Clone`) for each connection and
/// spawns a separate task to handle it. If your handler accesses shared
/// mutable state, use synchronization primitives like `Arc<Mutex<T>>` or
/// message-passing channels.
///
/// For serial execution of commands, implement queuing/routing logic within
/// your handler using channels or a task queue.
///
/// # Example
///
/// ```rust,no_run
/// use daemon_cli::prelude::*;
/// use tokio::io::{AsyncWrite, AsyncWriteExt};
///
/// #[derive(Clone)]
/// struct CommandProcessor;
///
/// #[async_trait]
/// impl CommandHandler for CommandProcessor {
///     async fn handle(
///         &self,
///         command: &str,
///         mut output: impl AsyncWrite + Send + Unpin,
///         cancel_token: CancellationToken,
///     ) -> Result<()> {
///         // Parse the command
///         let parts: Vec<&str> = command.trim().split_whitespace().collect();
///
///         match parts.get(0) {
///             Some(&"process") => {
///                 // Stream output as it's generated
///                 output.write_all(b"Processing...\n").await?;
///
///                 // Check for cancellation during long operations
///                 if cancel_token.is_cancelled() {
///                     return Err(anyhow::anyhow!("Operation cancelled"));
///                 }
///
///                 output.write_all(b"Done!\n").await?;
///                 Ok(())
///             }
///             Some(&"status") => {
///                 output.write_all(b"Ready\n").await?;
///                 Ok(())
///             }
///             _ => {
///                 Err(anyhow::anyhow!("Unknown command"))
///             }
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Process a command with streaming output and cancellation support.
    ///
    /// This method may be called concurrently from multiple tasks. Ensure
    /// your implementation is thread-safe if accessing shared state.
    ///
    /// Write output incrementally via `output`. Long-running operations should
    /// check `cancel_token.is_cancelled()` to handle graceful cancellation.
    async fn handle(
        &self,
        command: &str,
        output: impl AsyncWrite + Send + Unpin,
        cancel_token: CancellationToken,
    ) -> Result<()>;
}
