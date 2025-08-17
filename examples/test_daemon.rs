use daemon_rpc::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone)]
struct TestDaemon;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
enum TestMethod {
    ProcessFile { path: PathBuf },
    GetStatus,
    LongRunningTask { duration_ms: u64 },
}

impl RpcMethod for TestMethod {
    type Response = String;
}

#[async_trait]
impl RpcHandler<TestMethod> for TestDaemon {
    async fn handle(
        &mut self,
        method: TestMethod,
        cancel_token: CancellationToken,
        status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
    ) -> Result<String> {
        match method {
            TestMethod::ProcessFile { path } => {
                status_tx
                    .send(DaemonStatus::Busy(format!("Processing file: {:?}", path)))
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                tokio::time::sleep(Duration::from_millis(10)).await;

                status_tx
                    .send(DaemonStatus::Ready)
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                Ok(format!("Processed file: {:?}", path))
            }
            TestMethod::GetStatus => Ok("Ready".to_string()),
            TestMethod::LongRunningTask { duration_ms } => {
                status_tx
                    .send(DaemonStatus::Busy("Starting long-running task".to_string()))
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                let chunks = duration_ms / 10;
                for i in 0..chunks {
                    if cancel_token.is_cancelled() {
                        status_tx
                            .send(DaemonStatus::Ready)
                            .await
                            .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                        return Err(anyhow::anyhow!("Task cancelled"));
                    }

                    tokio::time::sleep(Duration::from_millis(10)).await;

                    if i % 10 == 0 {
                        let progress = (i * 100) / chunks;
                        status_tx
                            .send(DaemonStatus::Busy(format!(
                                "Long-running task: {}% complete",
                                progress
                            )))
                            .await
                            .map_err(|_| anyhow::anyhow!("Failed to send status"))?;
                    }
                }

                status_tx
                    .send(DaemonStatus::Ready)
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                Ok("Long-running task completed".to_string())
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Parse command line arguments
    let mut daemon_id = None;
    let mut build_timestamp = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--daemon-id" => {
                if i + 1 < args.len() {
                    daemon_id = Some(
                        args[i + 1]
                            .parse::<u64>()
                            .map_err(|_| anyhow::anyhow!("Invalid daemon-id"))?,
                    );
                    i += 2;
                } else {
                    return Err(anyhow::anyhow!("--daemon-id requires a value"));
                }
            }
            "--build-timestamp" => {
                if i + 1 < args.len() {
                    build_timestamp = Some(
                        args[i + 1]
                            .parse::<u64>()
                            .map_err(|_| anyhow::anyhow!("Invalid build-timestamp"))?,
                    );
                    i += 2;
                } else {
                    return Err(anyhow::anyhow!("--build-timestamp requires a value"));
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    let daemon_id = daemon_id.ok_or_else(|| anyhow::anyhow!("--daemon-id is required"))?;
    let build_timestamp = build_timestamp.unwrap_or(1234567890); // Default for testing

    println!(
        "Starting test daemon with ID: {} (build timestamp: {})",
        daemon_id, build_timestamp
    );

    // Create and start the daemon server
    let daemon = TestDaemon;
    let server = DaemonServer::new(daemon_id, build_timestamp, daemon);

    // Run the socket server (this will block until shutdown)
    server.spawn_with_socket().await?;

    Ok(())
}
