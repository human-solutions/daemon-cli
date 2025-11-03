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
//! - **Version Management**: Automatic daemon restart when binary is rebuilt (mtime-based)
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
//!     ) -> Result<i32> {
//!         // Parse and process the command
//!         output.write_all(b"Processing: ").await?;
//!         output.write_all(command.as_bytes()).await?;
//!         output.write_all(b"\n").await?;
//!         Ok(0)
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
//! #     ) -> Result<i32> {
//! #         output.write_all(command.as_bytes()).await?;
//! #         Ok(0)
//! #     }
//! # }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let handler = MyHandler;
//!     // Automatically detects daemon name and binary mtime
//!     let (server, _handle) = DaemonServer::new("/path/to/project", handler);
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
use std::{env, fs, time::UNIX_EPOCH};
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

/// Get the modification time of the current executable binary.
///
/// Returns the executable's mtime as seconds since Unix epoch. This is used
/// internally by `DaemonServer::new()` and `DaemonClient::connect()` for
/// automatic version checking.
///
/// When the binary is rebuilt, its mtime changes, allowing the daemon to
/// automatically restart on the next client connection. This approach ensures
/// version synchronization works across all crates in your workspace,
/// regardless of which one changed.
///
/// # Panics
///
/// Panics if the current executable path cannot be determined, the file metadata
/// cannot be read, or the modification time is before the Unix epoch.
///
/// # Example
///
/// ```rust
/// use daemon_cli::get_build_timestamp;
///
/// let timestamp = get_build_timestamp();
/// println!("Binary was last modified at: {}", timestamp);
/// ```
pub fn get_build_timestamp() -> u64 {
    let exe_path = env::current_exe().expect("Failed to get current executable path");
    let metadata = fs::metadata(&exe_path).expect("Failed to get executable metadata");
    let mtime = metadata
        .modified()
        .expect("Failed to get executable modification time");

    mtime
        .duration_since(UNIX_EPOCH)
        .expect("Modification time before UNIX epoch")
        .as_secs()
}

fn auto_detect_daemon_name() -> String {
    env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "daemon".to_string())
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
/// mutable state, use synchronization primitives like [`Arc<Mutex<T>>`](std::sync::Arc) or
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
///     ) -> Result<i32> {
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
///                 Ok(0)
///             }
///             Some(&"status") => {
///                 output.write_all(b"Ready\n").await?;
///                 Ok(0)
///             }
///             _ => {
///                 output.write_all(b"Unknown command\n").await?;
///                 Ok(127)  // Exit code 127 for unknown command
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
    ///
    /// Returns an exit code (0 for success, 1-255 for errors). For unrecoverable
    /// errors, return `Err(e)` which will be reported as exit code 1.
    async fn handle(
        &self,
        command: &str,
        output: impl AsyncWrite + Send + Unpin,
        cancel_token: CancellationToken,
    ) -> Result<i32>;
}
