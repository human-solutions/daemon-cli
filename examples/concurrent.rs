//! Example: Concurrent Command Processing with Shared State
//!
//! This example demonstrates how to build a daemon handler that safely
//! handles concurrent client connections with shared mutable state.
//!
//! Features:
//! - Thread-safe shared state using Arc<Mutex<T>>
//! - Concurrent command execution from multiple clients
//! - Request counting and statistics
//! - Command queue for work tracking
//!
//! Usage:
//! ```bash
//! # Terminal 1: Start the daemon
//! cargo run --example concurrent -- daemon
//!
//! # Terminal 2-5: Send concurrent commands from same directory
//! echo "add-task Build feature X" | cargo run --example concurrent
//! echo "add-task Write tests" | cargo run --example concurrent
//! echo "stats" | cargo run --example concurrent
//! echo "list-tasks" | cargo run --example concurrent
//! ```

use anyhow::Result;
use daemon_cli::prelude::*;
use std::{collections::VecDeque, env, sync::Arc};
use tokio::{
    io::{self, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::Mutex,
    time::{Duration, sleep},
};

/// Shared state for the task queue daemon
#[derive(Clone)]
struct TaskQueueHandler {
    state: Arc<Mutex<TaskQueueState>>,
}

/// The actual shared state - protected by Mutex for thread-safe concurrent access
struct TaskQueueState {
    tasks: VecDeque<String>,
    total_requests: usize,
    active_requests: usize,
}

impl TaskQueueHandler {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TaskQueueState {
                tasks: VecDeque::new(),
                total_requests: 0,
                active_requests: 0,
            })),
        }
    }
}

#[async_trait]
impl CommandHandler for TaskQueueHandler {
    async fn handle(
        &self,
        command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        cancel_token: CancellationToken,
    ) -> Result<i32> {
        // Increment active requests counter
        {
            let mut state = self.state.lock().await;
            state.total_requests += 1;
            state.active_requests += 1;
        }

        // Ensure we decrement on exit (even if error occurs)
        let state_clone = self.state.clone();
        let _guard = scopeguard::guard((), move |_| {
            // Use block_in_place to safely acquire blocking lock in Drop
            // This ensures the decrement always happens, even under contention
            // Safe because the critical section is tiny
            tokio::task::block_in_place(|| {
                let mut state = state_clone.blocking_lock();
                state.active_requests = state.active_requests.saturating_sub(1);
            });
        });

        let parts: Vec<&str> = command.trim().split_whitespace().collect();

        match parts.get(0) {
            Some(&"add-task") => {
                let task_description = parts[1..].join(" ");
                if task_description.is_empty() {
                    return Err(anyhow::anyhow!("Task description required"));
                }

                // Add task to queue
                {
                    let mut state = self.state.lock().await;
                    state.tasks.push_back(task_description.clone());
                }

                output
                    .write_all(format!("Added task: {}\n", task_description).as_bytes())
                    .await?;
                Ok(0)
            }

            Some(&"list-tasks") => {
                let tasks = {
                    let state = self.state.lock().await;
                    state.tasks.clone()
                };

                if tasks.is_empty() {
                    output.write_all(b"No tasks in queue\n").await?;
                } else {
                    output
                        .write_all(format!("Tasks in queue ({})\n", tasks.len()).as_bytes())
                        .await?;
                    for (i, task) in tasks.iter().enumerate() {
                        output
                            .write_all(format!("  {}. {}\n", i + 1, task).as_bytes())
                            .await?;
                    }
                }
                Ok(0)
            }

            Some(&"process-task") => {
                // Pop a task from the queue
                let task = {
                    let mut state = self.state.lock().await;
                    state.tasks.pop_front()
                };

                if let Some(task) = task {
                    output
                        .write_all(format!("Processing task: {}\n", task).as_bytes())
                        .await?;

                    // Simulate work with cancellation support
                    for i in 0..10 {
                        if cancel_token.is_cancelled() {
                            output.write_all(b"Processing cancelled\n").await?;
                            return Err(anyhow::anyhow!("Processing cancelled"));
                        }
                        sleep(Duration::from_millis(100)).await;
                        output
                            .write_all(format!("Progress: {}%\n", (i + 1) * 10).as_bytes())
                            .await?;
                    }

                    output.write_all(b"Task completed!\n").await?;
                } else {
                    output.write_all(b"No tasks to process\n").await?;
                }
                Ok(0)
            }

            Some(&"stats") => {
                let (total, active, queue_len) = {
                    let state = self.state.lock().await;
                    (
                        state.total_requests,
                        state.active_requests,
                        state.tasks.len(),
                    )
                };

                output.write_all(b"=== Task Queue Statistics ===\n").await?;
                output
                    .write_all(format!("Total requests:  {}\n", total).as_bytes())
                    .await?;
                output
                    .write_all(format!("Active requests: {}\n", active).as_bytes())
                    .await?;
                output
                    .write_all(format!("Tasks in queue:  {}\n", queue_len).as_bytes())
                    .await?;
                Ok(0)
            }

            Some(&"clear") => {
                let count = {
                    let mut state = self.state.lock().await;
                    let count = state.tasks.len();
                    state.tasks.clear();
                    count
                };
                output
                    .write_all(format!("Cleared {} tasks\n", count).as_bytes())
                    .await?;
                Ok(0)
            }

            _ => {
                output.write_all(b"Available commands:\n").await?;
                output
                    .write_all(b"  add-task <description>  - Add a task to the queue\n")
                    .await?;
                output
                    .write_all(b"  list-tasks              - Show all queued tasks\n")
                    .await?;
                output
                    .write_all(b"  process-task            - Process next task from queue\n")
                    .await?;
                output
                    .write_all(b"  stats                   - Show daemon statistics\n")
                    .await?;
                output
                    .write_all(b"  clear                   - Clear all tasks\n")
                    .await?;
                Ok(127)  // Exit code 127 for unknown command
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Check if first argument is "daemon", otherwise run as client
    if args.len() >= 2 && args[1] == "daemon" {
        run_daemon_mode().await
    } else {
        run_client_mode().await
    }
}

fn print_usage() {
    println!("Concurrent Task Queue Example");
    println!("============================");
    println!("Demonstrates concurrent command handling with shared state");
    println!();
    println!("Usage: cargo run --example concurrent -- [mode]");
    println!();
    println!("Modes:");
    println!("  daemon         Start daemon server");
    println!("  (default)      Run as client");
    println!();
    println!("Note: Both daemon and client use current directory as scope");
    println!();
    println!("Examples:");
    println!("  # Start daemon");
    println!("  cargo run --example concurrent -- daemon");
    println!();
    println!("  # Send commands (from same directory as daemon)");
    println!("  echo \"add-task Build feature\" | cargo run --example concurrent");
    println!("  echo \"stats\" | cargo run --example concurrent");
    println!("  echo \"list-tasks\" | cargo run --example concurrent");
}

async fn run_daemon_mode() -> Result<()> {
    let root_path = env::current_dir()?.to_string_lossy().to_string();

    // Initialize tracing subscriber for daemon logs
    // Logs go to stderr with compact format
    // To redirect to a file instead:
    //   let file = std::fs::File::create("/tmp/daemon.log")?;
    //   tracing_subscriber::fmt().with_writer(file).init();
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .compact()
        .init();

    tracing::info!(
        root_path,
        "Starting task queue daemon with concurrent request handling"
    );

    let handler = TaskQueueHandler::new();
    // Automatically detects daemon name and binary mtime
    let (server, _handle) = DaemonServer::new(&root_path, handler);
    server.run().await?;

    Ok(())
}

async fn run_client_mode() -> Result<()> {
    let root_path = env::current_dir()?.to_string_lossy().to_string();

    // Read command from stdin
    let mut stdin = io::stdin();
    let mut command = String::new();

    stdin.read_to_string(&mut command).await?;

    if command.trim().is_empty() {
        eprintln!("Error: No command provided via stdin");
        print_usage();
        std::process::exit(1);
    }

    // Connect to daemon (auto-spawns if needed, auto-detects everything)
    let mut client = DaemonClient::connect(&root_path).await?;

    // Execute command and stream output to stdout
    let exit_code = client.execute_command(command).await?;

    // Exit with the command's exit code
    std::process::exit(exit_code);
}
