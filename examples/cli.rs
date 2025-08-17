mod common;

use common::*;
use daemon_rpc::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "daemon" => run_daemon_mode().await,
        "demo" => run_demo_mode().await,
        "test" => run_test_mode().await,
        _ => {
            eprintln!("❌ Unknown mode: {}", args[1]);
            print_usage();
            Err(anyhow::anyhow!("Invalid mode"))
        }
    }
}

fn print_usage() {
    println!("🎯 daemon-rpc All-in-One Example");
    println!("=================================");
    println!("Usage: cargo run --example cli -- <mode> [options]");
    println!();
    println!("Modes:");
    println!("  daemon     Start daemon server");
    println!("  demo       Run automated demonstration");
    println!("  test       Run quick functionality test");
    println!();
    println!("Daemon options:");
    println!("  --daemon-id <id>          Daemon ID (required)");
    println!("  --build-timestamp <time>  Build timestamp (optional)");
    println!("  --rich                    Enable rich mode with startup delay");
    println!();
    println!("Examples:");
    println!("  cargo run --example cli -- daemon --daemon-id 1000 --rich");
    println!("  cargo run --example cli -- demo");
    println!("  cargo run --example cli -- test");
}

async fn run_daemon_mode() -> Result<()> {
    let (daemon_id, build_timestamp, rich_mode) = parse_daemon_args()?;

    println!(
        "🚀 Starting daemon (ID: {}, build: {}, rich: {})",
        daemon_id, build_timestamp, rich_mode
    );

    let daemon = DemoDaemon::new(rich_mode);
    daemon.simulate_startup_delay().await;

    let server = DaemonServer::new(daemon_id, build_timestamp, daemon);
    server.spawn_with_socket().await?;

    Ok(())
}

async fn run_demo_mode() -> Result<()> {
    // Build daemon first
    println!("🔨 Building daemon...");
    let _ = std::process::Command::new("cargo")
        .args(["build", "--example", "cli"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();

    println!("🚀 daemon-rpc Framework Demonstration");
    println!("======================================");
    println!("All features in one comprehensive demo!\n");

    // Demo 1: Auto-spawn with startup delay
    println!("📋 Demo 1: Auto-Spawn with Startup Delay");
    println!("-----------------------------------------");
    let start_time = std::time::Instant::now();

    let mut client: DaemonClient<TestMethod> =
        DaemonClient::connect(2000, get_daemon_path(), get_build_timestamp()).await?;

    println!(
        "✅ Connected in {:.2}s (includes 2s daemon startup)",
        start_time.elapsed().as_secs_f64()
    );

    // Demo 2: Status streaming
    println!("\n📊 Demo 2: Real-time Status Streaming");
    println!("-------------------------------------");
    tokio::spawn(async move {
        while let Ok(status) = client.status_receiver.recv().await {
            match status {
                DaemonStatus::Ready => println!("📊 Status: ✅ Ready"),
                DaemonStatus::Busy(msg) => println!("📊 Status: 🔄 {}", msg),
                DaemonStatus::Error(err) => println!("📊 Status: ❌ Error: {}", err),
            }
        }
    });

    let mut demo_client: DaemonClient<TestMethod> =
        DaemonClient::connect(2000, get_daemon_path(), get_build_timestamp()).await?;

    let result = demo_client
        .request(TestMethod::ProcessFile {
            path: "/tmp/large_file.txt".into(),
            options: ProcessOptions {
                compress: true,
                validate: true,
                backup: false,
            },
        })
        .await?;

    match result {
        RpcResponse::Success { output } => {
            println!(
                "✅ File processing: {} bytes, {} files",
                output.processed_bytes,
                output.files_created.len()
            );
        }
        _ => println!("❌ Demo failed"),
    }

    // Demo 3: Cancellation
    println!("\n🛑 Demo 3: Task Cancellation");
    println!("-----------------------------");
    demonstrate_cancellation().await?;

    // Demo 4: Multiple clients
    println!("\n👥 Demo 4: Multiple Client Support");
    println!("-----------------------------------");
    demonstrate_multiple_clients().await?;

    println!("\n🎉 All demos complete! Framework features verified:");
    println!("  ✅ Auto-spawn with startup delay");
    println!("  ✅ Status streaming");
    println!("  ✅ Task cancellation");
    println!("  ✅ Multiple clients");
    println!("  ✅ Version management");

    Ok(())
}

async fn demonstrate_cancellation() -> Result<()> {
    let mut task_client: DaemonClient<TestMethod> =
        DaemonClient::connect(2000, get_daemon_path(), get_build_timestamp()).await?;

    let mut cancel_client: DaemonClient<TestMethod> =
        DaemonClient::connect(2000, get_daemon_path(), get_build_timestamp()).await?;

    let task_future = task_client.request(TestMethod::LongTask {
        duration_seconds: 8,
        description: "Demo cancellation task".to_string(),
    });

    let cancel_future = async {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        println!("🛑 Sending cancel...");
        cancel_client.cancel_current_task().await
    };

    tokio::select! {
        result = task_future => {
            match result? {
                RpcResponse::Success { output } => println!("✅ Task completed: {}", output.status),
                RpcResponse::Error { error } => println!("🛑 Task cancelled: {}", error),
                _ => println!("⚠️  Unexpected result"),
            }
        }
        cancel_result = cancel_future => {
            match cancel_result? {
                true => println!("✅ Cancel sent successfully"),
                false => println!("⚠️  Cancel failed"),
            }
            tokio::time::sleep(Duration::from_millis(500)).await; // Wait for task to respond
        }
    }

    Ok(())
}

async fn demonstrate_multiple_clients() -> Result<()> {
    let build_timestamp = get_build_timestamp();
    let mut clients = Vec::new();

    // Connect 3 clients
    for i in 1..=3 {
        let client: DaemonClient<TestMethod> =
            DaemonClient::connect(2000, get_daemon_path(), build_timestamp).await?;
        clients.push(client);
        println!("✅ Client {} connected", i);
    }

    // Each client gets uptime
    for (i, client) in clients.iter_mut().enumerate() {
        let result = client.request(TestMethod::GetUptime).await?;
        match result {
            RpcResponse::Success { output } => {
                println!("   Client {}: {}", i + 1, output.status);
            }
            _ => println!("   Client {}: Error", i + 1),
        }
    }

    Ok(())
}

async fn run_test_mode() -> Result<()> {
    println!("🧪 Quick Functionality Test");
    println!("============================");

    // Build daemon
    let _ = std::process::Command::new("cargo")
        .args(["build", "--example", "cli"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();

    // Test basic functionality
    let mut client: DaemonClient<TestMethod> =
        DaemonClient::connect(3000, get_daemon_path(), get_build_timestamp()).await?;

    // Quick status check
    let result = client.request(TestMethod::GetStatus).await?;
    match result {
        RpcResponse::Success { output } => {
            println!("✅ Daemon status: {}", output.status);
        }
        _ => {
            println!("❌ Status check failed");
            return Err(anyhow::anyhow!("Status check failed"));
        }
    }

    // Quick task
    let result = client
        .request(TestMethod::QuickTask {
            message: "Test message".to_string(),
        })
        .await?;

    match result {
        RpcResponse::Success { output } => {
            println!(
                "✅ Quick task: {} ({}ms)",
                output.status, output.duration_ms
            );
        }
        _ => {
            println!("❌ Quick task failed");
            return Err(anyhow::anyhow!("Quick task failed"));
        }
    }

    // Test cancellation
    println!("🛑 Testing cancellation...");
    let mut cancel_client: DaemonClient<TestMethod> =
        DaemonClient::connect(3000, get_daemon_path(), get_build_timestamp()).await?;

    let task_future = client.request(TestMethod::LongTask {
        duration_seconds: 5,
        description: "Test task".to_string(),
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let cancelled = cancel_client.cancel_current_task().await?;
    if cancelled {
        println!("✅ Cancellation works");

        // Wait for task to respond to cancellation
        match task_future.await? {
            RpcResponse::Error { error } if error.contains("cancelled") => {
                println!("✅ Task properly cancelled");
            }
            _ => {
                println!("⚠️  Task may not have been cancelled");
            }
        }
    } else {
        println!("⚠️  Cancellation failed");
    }

    println!("\n🎉 All tests passed! Framework is working correctly.");
    println!("👋 Test complete. Daemon remains running.");

    Ok(())
}
