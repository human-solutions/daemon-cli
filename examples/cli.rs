mod common;

use anyhow::{Result, bail};
use common::*;
use daemon_rpc::prelude::*;
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
    println!("daemon-rpc stdin/stdout Example");
    println!("================================");
    println!("Usage: cargo run --example cli -- <mode> [options]");
    println!();
    println!("Modes:");
    println!("  daemon         Start daemon server");
    println!("  (any other)    Run as client (reads stdin, sends to daemon, outputs to stdout)");
    println!();
    println!("Daemon options:");
    println!("  --daemon-id <id>          Daemon ID (required)");
    println!("  --build-timestamp <time>  Build timestamp (optional)");
    println!();
    println!("Examples:");
    println!("  # Start daemon");
    println!("  cargo run --example cli -- daemon --daemon-id 1000");
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
    let (daemon_id, build_timestamp) = parse_daemon_args()?;

    println!(
        "Starting daemon (ID: {}, build: {})",
        daemon_id, build_timestamp
    );

    let handler = CommandProcessor::new();
    let server = DaemonServer::new(daemon_id, build_timestamp, handler);
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

    // Connect to daemon (auto-spawns if needed)
    let daemon_id = 1000;
    let daemon_exe = get_daemon_path();
    let build_timestamp = get_build_timestamp();

    let mut client = DaemonClient::connect(daemon_id, daemon_exe, build_timestamp).await?;

    // Execute command and stream output to stdout
    client.execute_command(command).await?;

    Ok(())
}
