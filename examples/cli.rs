mod common;

use anyhow::Result;
use common::*;
use daemon_cli::prelude::*;
use std::env;
use tokio::io::{self, AsyncReadExt};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Check if first argument is "daemon" or "--stop", otherwise run as client
    if args.len() >= 2 && args[1] == "daemon" {
        run_daemon_mode().await
    } else if args.len() >= 2 && args[1] == "--stop" {
        run_stop_mode().await
    } else {
        run_client_mode().await
    }
}

fn print_usage() {
    println!("daemon-cli stdin/stdout Example");
    println!("================================");
    println!("Usage: cargo run --example cli -- [mode]");
    println!();
    println!("Modes:");
    println!("  daemon         Start daemon server");
    println!("  --stop         Force-stop the running daemon");
    println!("  (default)      Run as client (reads stdin, sends to daemon, outputs to stdout)");
    println!();
    println!("Note: Both daemon and client use current directory as scope");
    println!();
    println!("Examples:");
    println!("  # Start daemon");
    println!("  cargo run --example cli -- daemon");
    println!();
    println!("  # Execute commands via client");
    println!("  echo \"status\" | cargo run --example cli");
    println!("  echo \"process file.txt\" | cargo run --example cli");
    println!("  echo \"long 5\" | cargo run --example cli");
    println!();
    println!("  # Force-stop the daemon");
    println!("  cargo run --example cli -- --stop");
    println!();
    println!("Available commands:");
    println!("  status              - Get daemon status");
    println!("  uptime              - Get daemon uptime");
    println!("  process [file]      - Process a file (simulated)");
    println!("  long [seconds]      - Long-running task (test cancellation with Ctrl+C)");
    println!("  echo [message]      - Echo a message");
}

async fn run_stop_mode() -> Result<()> {
    let root_path = env::current_dir()?.to_string_lossy().to_string();

    // Connect to daemon to get access to force_stop method
    let client = DaemonClient::connect(&root_path).await?;

    println!("Stopping daemon...");
    client.force_stop().await?;
    println!("Daemon stopped successfully");

    Ok(())
}

async fn run_daemon_mode() -> Result<()> {
    // Parse daemon arguments: daemon --daemon-name X --root-path Y --startup-reason Z
    let args: Vec<String> = env::args().collect();
    let mut root_path = env::current_dir()?.to_string_lossy().to_string();
    let mut startup_reason = StartupReason::default();

    // Simple argument parsing
    let mut i = 2; // Skip program name and "daemon"
    while i < args.len() {
        match args[i].as_str() {
            "--daemon-name" => {
                // Skip daemon name - DaemonServer auto-detects it
                i += 2;
            }
            "--root-path" => {
                if i + 1 < args.len() {
                    root_path = args[i + 1].clone();
                }
                i += 2;
            }
            "--startup-reason" => {
                if i + 1 < args.len() {
                    startup_reason = args[i + 1].parse().unwrap_or_default();
                }
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

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

    tracing::info!(root_path, startup_reason = %startup_reason, "Starting daemon");

    let handler = CommandProcessor::new();
    // Automatically detects daemon name and binary mtime
    let (server, _handle) = DaemonServer::new(&root_path, handler, startup_reason);
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
