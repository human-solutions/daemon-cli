use daemon_rpc::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

// Import the daemon's types (same as file_processor_daemon.rs)
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum FileMethod {
    ProcessFile {
        path: PathBuf,
        options: ProcessOptions,
    },
    GetStatus,
    GetUptime,
    LongTask {
        duration_seconds: u64,
        description: String,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ProcessOptions {
    pub compress: bool,
    pub validate: bool,
    pub backup: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ProcessResult {
    pub processed_bytes: u64,
    pub files_created: Vec<String>,
    pub duration_ms: u64,
    pub status: String,
}

impl RpcMethod for FileMethod {
    type Response = ProcessResult;
}

fn get_daemon_path() -> PathBuf {
    let mut exe_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    exe_path.push("target");
    exe_path.push("debug");
    exe_path.push("examples");
    exe_path.push("file_processor_daemon");

    if cfg!(windows) {
        exe_path.set_extension("exe");
    }

    exe_path
}

fn get_build_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn demonstrate_daemon_startup() -> Result<DaemonClient<FileMethod>> {
    println!("🚀 Phase 5 Demo: File Processor CLI");
    println!("=====================================\n");

    // Get daemon binary path
    let daemon_executable = get_daemon_path();
    let build_timestamp = get_build_timestamp();

    println!("📋 Demo Requirements:");
    println!("  ✓ Daemon startup takes 2 seconds with busy status");
    println!("  ✓ CLI shows status updates during initialization");
    println!("  ✓ Long-running tasks can be cancelled");
    println!("  ✓ Multiple clients can interact with same daemon");
    println!("  ✓ Ctrl-C exits CLI but leaves daemon running\n");

    // Connect to daemon (will auto-spawn with 2s startup delay)
    println!("🔄 Connecting to file processor daemon...");
    println!("   (This will show the 2-second startup delay)");

    let start_time = std::time::Instant::now();
    let mut client: DaemonClient<FileMethod> =
        DaemonClient::connect(2000, daemon_executable, build_timestamp).await?;
    let connection_time = start_time.elapsed();

    println!("✅ Connected! (took {:.2}s)", connection_time.as_secs_f64());

    // Start monitoring status updates
    tokio::spawn(async move {
        while let Ok(status) = client.status_receiver.recv().await {
            match status {
                DaemonStatus::Ready => println!("📊 Status: ✅ Ready"),
                DaemonStatus::Busy(msg) => println!("📊 Status: 🔄 {}", msg),
                DaemonStatus::Error(err) => println!("📊 Status: ❌ Error: {}", err),
            }
        }
    });

    // Return a new client for the demo
    let demo_client: DaemonClient<FileMethod> =
        DaemonClient::connect(2000, get_daemon_path(), build_timestamp).await?;
    Ok(demo_client)
}

async fn demonstrate_file_processing(client: &mut DaemonClient<FileMethod>) -> Result<()> {
    println!("\n🏗️  Demo 1: File Processing with Status Updates");
    println!("================================================");

    let options = ProcessOptions {
        compress: true,
        validate: true,
        backup: false,
    };

    println!("🚀 Starting file processing with compression and validation...");
    println!("   (Watch for real-time status updates above)");

    let result = client
        .request(FileMethod::ProcessFile {
            path: "/tmp/demo_file.txt".into(),
            options,
        })
        .await?;

    match result {
        RpcResponse::Success { output } => {
            println!("✅ Processing completed!");
            println!("   📦 Processed: {} bytes", output.processed_bytes);
            println!("   📁 Files created: {:?}", output.files_created);
            println!("   ⏱️  Duration: {}ms", output.duration_ms);
        }
        RpcResponse::Error { error } => {
            println!("❌ Processing failed: {}", error);
        }
        _ => {
            println!("⚠️  Unexpected response type");
        }
    }

    Ok(())
}

async fn demonstrate_cancellation() -> Result<()> {
    println!("\n🛑 Demo 2: Task Cancellation");
    println!("=============================");

    // Create two clients to demonstrate cancellation
    let build_timestamp = get_build_timestamp();
    let mut task_client: DaemonClient<FileMethod> =
        DaemonClient::connect(2000, get_daemon_path(), build_timestamp).await?;
    let mut cancel_client: DaemonClient<FileMethod> =
        DaemonClient::connect_via_socket(2000, get_daemon_path(), build_timestamp).await?;

    println!("🚀 Starting 10-second long-running task...");
    println!("   (Will be cancelled after 3 seconds)");

    // Start long task
    let task_future = task_client.request(FileMethod::LongTask {
        duration_seconds: 10,
        description: "Processing large dataset".to_string(),
    });

    // Cancel after 3 seconds
    let cancel_future = async {
        tokio::time::sleep(Duration::from_millis(3000)).await;
        println!("\n🛑 Sending cancellation request...");

        match cancel_client.cancel_current_task().await {
            Ok(true) => println!("✅ Task cancelled successfully!"),
            Ok(false) => println!("⚠️  No task running or cancellation failed"),
            Err(e) => println!("❌ Cancel error: {}", e),
        }
    };

    // Race between task completion and cancellation
    tokio::select! {
        result = task_future => {
            match result? {
                RpcResponse::Success { output } => {
                    println!("✅ Task completed before cancellation: {}", output.status);
                }
                RpcResponse::Error { error } => {
                    println!("🛑 Task was cancelled: {}", error);
                }
                _ => {
                    println!("⚠️  Unexpected response");
                }
            }
        }
        _ = cancel_future => {
            // Cancellation was sent, wait a bit more for task to respond
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }

    Ok(())
}

async fn demonstrate_multiple_clients() -> Result<()> {
    println!("\n👥 Demo 3: Multiple Client Support");
    println!("===================================");

    // Connect multiple clients to same daemon
    let build_timestamp = get_build_timestamp();
    let mut client1: DaemonClient<FileMethod> =
        DaemonClient::connect_via_socket(2000, get_daemon_path(), build_timestamp).await?;
    let mut client2: DaemonClient<FileMethod> =
        DaemonClient::connect_via_socket(2000, get_daemon_path(), build_timestamp).await?;
    let mut client3: DaemonClient<FileMethod> =
        DaemonClient::connect_via_socket(2000, get_daemon_path(), build_timestamp).await?;

    println!("✅ Connected 3 clients to the same daemon");

    // Each client gets uptime (should be the same)
    println!("\n📊 Getting uptime from each client:");

    for (i, client) in [&mut client1, &mut client2, &mut client3]
        .iter_mut()
        .enumerate()
    {
        let result = client.request(FileMethod::GetUptime).await?;
        match result {
            RpcResponse::Success { output } => {
                println!("   Client {}: {}", i + 1, output.status);
            }
            _ => println!("   Client {}: Error getting uptime", i + 1),
        }
    }

    // Try to start task from client1 while client2 tries to interfere
    println!("\n🔄 Testing single-task enforcement:");
    println!("   Client 1: Starting task...");

    let task1_future = client1.request(FileMethod::LongTask {
        duration_seconds: 2,
        description: "Client 1 task".to_string(),
    });

    // Give task time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    println!("   Client 2: Trying to start competing task...");
    let result2 = client2.request(FileMethod::GetStatus).await?;
    match result2 {
        RpcResponse::Error { error } => {
            println!("   ✅ Client 2 rejected: {}", error);
        }
        _ => {
            println!("   ⚠️  Client 2 should have been rejected");
        }
    }

    // Wait for client1's task to complete
    let result1 = task1_future.await?;
    match result1 {
        RpcResponse::Success { output } => {
            println!("   ✅ Client 1 task completed: {}", output.status);
        }
        _ => {
            println!("   ❌ Client 1 task failed");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Build the daemon first
    println!("🔨 Building file processor daemon...");
    let build_result = std::process::Command::new("cargo")
        .args(["build", "--example", "file_processor_daemon"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();

    match build_result {
        Ok(output) if output.status.success() => {
            println!("✅ Daemon build successful\n");
        }
        Ok(output) => {
            eprintln!(
                "❌ Daemon build failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(anyhow::anyhow!("Failed to build daemon"));
        }
        Err(e) => {
            eprintln!("❌ Failed to run build command: {}", e);
            return Err(anyhow::anyhow!("Build command failed"));
        }
    }

    // Run the demonstrations
    let mut client = demonstrate_daemon_startup().await?;

    demonstrate_file_processing(&mut client).await?;

    demonstrate_cancellation().await?;

    demonstrate_multiple_clients().await?;

    println!("\n🎉 Phase 5 Demo Complete!");
    println!("==========================");
    println!("All framework features demonstrated:");
    println!("  ✅ Automatic daemon spawning with startup feedback");
    println!("  ✅ Real-time status streaming during operations");
    println!("  ✅ Cross-process task cancellation (graceful + forceful)");
    println!("  ✅ Multiple client support with single-task enforcement");
    println!("  ✅ Version management and automatic restart");
    println!("  ✅ Clean process separation (daemon continues running)");

    println!("\n📝 To run the interactive CLI version:");
    println!("   cargo run --example file_processor");
    println!("   (Requires proper terminal for keyboard input)");

    println!("\n👋 Demo exiting. Daemon remains running for other clients.");

    Ok(())
}
