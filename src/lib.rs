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
//!         ctx: CommandContext,
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
//! #         _ctx: CommandContext,
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
//!     // startup_reason is passed from client via --startup-reason CLI arg
//!     let (server, _handle) = DaemonServer::new("/path/to/project", handler, StartupReason::default());
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
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, fs, str::FromStr, time::UNIX_EPOCH};
use tokio::io::AsyncWrite;
use tokio_util::sync::CancellationToken;

mod client;
mod error_context;
mod process;
mod server;
mod terminal;
mod transport;

pub use client::DaemonClient;
pub use error_context::ErrorContextBuffer;
pub use server::{DaemonHandle, DaemonServer};
pub use terminal::{ColorSupport, TerminalInfo, Theme};

/// Configuration for filtering which environment variables to pass from client to daemon.
///
/// By default, no environment variables are passed. Use [`EnvVarFilter::with_names`] to
/// specify exact variable names to include.
///
/// # Example
///
/// ```rust
/// use daemon_cli::EnvVarFilter;
///
/// // Pass specific env vars
/// let filter = EnvVarFilter::with_names(["MY_APP_DEBUG", "MY_APP_CONFIG"]);
///
/// // Or build incrementally
/// let filter = EnvVarFilter::none()
///     .include("MY_APP_DEBUG")
///     .include("MY_APP_CONFIG");
/// ```
#[derive(Debug, Clone, Default)]
pub struct EnvVarFilter {
    names: Vec<String>,
}

impl EnvVarFilter {
    /// Create a filter that passes no environment variables (default).
    pub fn none() -> Self {
        Self { names: vec![] }
    }

    /// Create a filter that passes env vars with the specified exact names.
    pub fn with_names(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Include an env var name to pass.
    pub fn include(mut self, name: impl Into<String>) -> Self {
        self.names.push(name.into());
        self
    }

    /// Filter environment variables from the current process.
    ///
    /// Returns a HashMap containing only the env vars whose names match
    /// those configured in this filter.
    pub fn filter_current_env(&self) -> HashMap<String, String> {
        if self.names.is_empty() {
            return HashMap::new();
        }
        self.names
            .iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| (name.clone(), value)))
            .collect()
    }
}

/// Context information passed with each command execution.
///
/// This struct bundles metadata about the command execution environment,
/// including terminal information and environment variables. It is designed
/// for extensibility - new fields can be added in the future without breaking
/// the handler trait signature.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommandContext {
    /// Information about the client's terminal environment
    pub terminal_info: TerminalInfo,
    /// Environment variables passed from client (filtered by exact name match).
    /// Empty by default for backward compatibility.
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
}

impl CommandContext {
    /// Create a new CommandContext with terminal info only (no env vars).
    pub fn new(terminal_info: TerminalInfo) -> Self {
        Self {
            terminal_info,
            env_vars: HashMap::new(),
        }
    }

    /// Create a CommandContext with terminal info and environment variables.
    pub fn with_env(terminal_info: TerminalInfo, env_vars: HashMap<String, String>) -> Self {
        Self {
            terminal_info,
            env_vars,
        }
    }
}

/// Reason why daemon was started.
///
/// This is passed to [`CommandHandler::on_startup`] to indicate
/// the circumstances under which the daemon started. The client
/// determines the reason and passes it via `--startup-reason` CLI arg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupReason {
    /// Fresh start - daemon starting for first time in this location
    #[default]
    FirstStart,
    /// Binary was updated (mtime changed), old daemon was replaced
    BinaryUpdated,
    /// Previous daemon crashed or was killed unexpectedly
    Recovered,
    /// User explicitly called `restart()` on the client
    ForceRestarted,
}

impl StartupReason {
    /// Convert to CLI argument string value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FirstStart => "first-start",
            Self::BinaryUpdated => "binary-updated",
            Self::Recovered => "recovered",
            Self::ForceRestarted => "force-restarted",
        }
    }
}

impl FromStr for StartupReason {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "first-start" => Ok(Self::FirstStart),
            "binary-updated" => Ok(Self::BinaryUpdated),
            "recovered" => Ok(Self::Recovered),
            "force-restarted" => Ok(Self::ForceRestarted),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for StartupReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests;

/// Convenient re-exports for common daemon-cli types and traits.
///
/// Use `use daemon_cli::prelude::*;` to import all commonly needed items.
pub mod prelude {
    pub use crate::{
        ColorSupport, CommandContext, CommandHandler, DaemonClient, DaemonHandle, DaemonServer,
        EnvVarFilter, ErrorContextBuffer, StartupReason, TerminalInfo, Theme,
    };
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

// Re-export transport utilities for integration tests
pub use transport::{pid_path, socket_path};

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
        .as_millis() as u64
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
///         ctx: CommandContext,
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
    /// The `ctx` parameter contains information about the command execution
    /// environment including terminal info (width, height, color support) and
    /// any environment variables passed from the client. Use this to format
    /// output appropriately and access client-side configuration.
    ///
    /// Write output incrementally via `output`. Long-running operations should
    /// check `cancel_token.is_cancelled()` to handle graceful cancellation.
    ///
    /// Returns an exit code (0 for success, 1-255 for errors). For unrecoverable
    /// errors, return `Err(e)` which will be reported as exit code 1.
    async fn handle(
        &self,
        command: &str,
        ctx: CommandContext,
        output: impl AsyncWrite + Send + Unpin,
        cancel_token: CancellationToken,
    ) -> Result<i32>;

    /// Called once when the daemon starts, before accepting connections.
    ///
    /// Override this method to log the startup reason or perform
    /// initialization that depends on whether this is a fresh start
    /// or a restart.
    ///
    /// The default implementation does nothing.
    fn on_startup(&self, _reason: StartupReason) {}
}
