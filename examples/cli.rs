mod common;

use anyhow::{Result, bail};
use common::*;
use daemon_cli::prelude::*;
use std::env;
use tokio::io::{self, AsyncReadExt};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "daemon" => run_daemon_mode().await,
        _ => run_client_mode().await,
    }
}

fn print_usage() {
    println!("daemon-cli stdin/stdout Example");
    println!("================================");
    println!("Usage: cargo run --example cli -- <mode> [options]");
    println!();
    println!("Modes:");
    println!("  daemon         Start daemon server");
    println!("  (any other)    Run as client (reads stdin, sends to daemon, outputs to stdout)");
    println!();
    println!("Daemon options:");
    println!("  --daemon-path <path>      Daemon path/scope (required)");
    println!();
    println!("Examples:");
    println!("  # Start daemon");
    println!("  cargo run --example cli -- daemon --daemon-path /tmp/test");
    println!();
    println!("  # Execute commands via client");
    println!("  echo \"status\" | cargo run --example cli");
    println!("  echo \"process file.txt\" | cargo run --example cli");
    println!("  echo \"long 5\" | cargo run --example cli");
    println!();
    println!("Available commands:");
    println!("  status              - Get daemon status");
    println!("  uptime              - Get daemon uptime");
    println!("  process [file]      - Process a file (simulated)");
    println!("  long [seconds]      - Long-running task (test cancellation with Ctrl+C)");
    println!("  echo [message]      - Echo a message");
}

async fn run_daemon_mode() -> Result<()> {
    let daemon_path = parse_daemon_args()?;

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

    tracing::info!(daemon_path, "Starting daemon");

    let handler = CommandProcessor::new();
    // Automatically detects daemon name and binary mtime
    let (server, _handle) = DaemonServer::new(&daemon_path, handler);
    server.run().await?;

    Ok(())
}

async fn run_client_mode() -> Result<()> {
    // Read command from stdin
    let mut stdin = io::stdin();
    let mut command = String::new();

    stdin.read_to_string(&mut command).await?;

    if command.trim().is_empty() {
        eprintln!("Error: No command provided via stdin");
        print_usage();
        bail!("No command provided");
    }

    // Connect to daemon (auto-spawns if needed, auto-detects everything)
    let daemon_path = env::current_dir()?.to_string_lossy().to_string();

    let mut client = DaemonClient::connect(&daemon_path).await?;

    // Execute command and stream output to stdout
    client.execute_command(command).await?;

    Ok(())
}
