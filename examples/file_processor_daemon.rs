use daemon_rpc::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone)]
struct FileProcessorDaemon {
    startup_time: std::time::Instant,
}

impl FileProcessorDaemon {
    fn new() -> Self {
        Self {
            startup_time: std::time::Instant::now(),
        }
    }
}

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

#[async_trait]
impl RpcHandler<FileMethod> for FileProcessorDaemon {
    async fn handle(
        &mut self,
        method: FileMethod,
        cancel_token: CancellationToken,
        status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
    ) -> Result<ProcessResult> {
        match method {
            FileMethod::ProcessFile { path, options } => {
                let start_time = std::time::Instant::now();

                status_tx
                    .send(DaemonStatus::Busy(format!(
                        "Starting processing of {:?}",
                        path
                    )))
                    .await
                    .map_err(|_| anyhow::anyhow!("Failed to send status"))?;

                // Simulate file analysis phase
                for i in 0..20 {
                    if cancel_token.is_cancelled() {
                        status_tx.send(DaemonStatus::Ready).await.ok();
                        return Err(anyhow::anyhow!(
                            "File processing cancelled during analysis phase"
                        ));
                    }

                    tokio::time::sleep(Duration::from_millis(50)).await;

                    if i % 5 == 0 {
                        let progress = (i * 100) / 20;
                        status_tx
                            .send(DaemonStatus::Busy(format!(
                                "Analyzing {:?}: {}% complete",
                                path, progress
                            )))
                            .await
                            .ok();
                    }
                }

                // Simulate processing phase based on options
                let processing_steps = if options.compress { 30 } else { 15 };
                let processing_steps = if options.validate {
                    processing_steps + 10
                } else {
                    processing_steps
                };
                let processing_steps = if options.backup {
                    processing_steps + 5
                } else {
                    processing_steps
                };

                for i in 0..processing_steps {
                    if cancel_token.is_cancelled() {
                        status_tx.send(DaemonStatus::Ready).await.ok();
                        return Err(anyhow::anyhow!(
                            "File processing cancelled during processing phase"
                        ));
                    }

                    tokio::time::sleep(Duration::from_millis(100)).await;

                    if i % 3 == 0 {
                        let progress = (i * 100) / processing_steps;
                        let phase = if options.compress && i < 30 {
                            "compressing"
                        } else if options.validate && i < 40 {
                            "validating"
                        } else if options.backup {
                            "backing up"
                        } else {
                            "processing"
                        };

                        status_tx
                            .send(DaemonStatus::Busy(format!(
                                "Processing {:?}: {} {}%",
                                path, phase, progress
                            )))
                            .await
                            .ok();
                    }
                }

                status_tx.send(DaemonStatus::Ready).await.ok();

                let duration = start_time.elapsed();
                let mut files_created = vec![format!("{}.processed", path.display())];

                if options.compress {
                    files_created.push(format!("{}.gz", path.display()));
                }
                if options.backup {
                    files_created.push(format!("{}.backup", path.display()));
                }

                Ok(ProcessResult {
                    processed_bytes: 1024 * 1024 * 5, // Simulate 5MB processed
                    files_created,
                    duration_ms: duration.as_millis() as u64,
                    status: "Successfully processed file".to_string(),
                })
            }

            FileMethod::GetStatus => Ok(ProcessResult {
                processed_bytes: 0,
                files_created: vec![],
                duration_ms: 0,
                status: "Daemon ready for processing".to_string(),
            }),

            FileMethod::GetUptime => {
                let uptime = self.startup_time.elapsed();
                Ok(ProcessResult {
                    processed_bytes: 0,
                    files_created: vec![],
                    duration_ms: uptime.as_millis() as u64,
                    status: format!("Daemon uptime: {:.2}s", uptime.as_secs_f64()),
                })
            }

            FileMethod::LongTask {
                duration_seconds,
                description,
            } => {
                let start_time = std::time::Instant::now();

                status_tx
                    .send(DaemonStatus::Busy(format!("Starting: {}", description)))
                    .await
                    .ok();

                let total_steps = duration_seconds * 10; // 100ms per step
                for i in 0..total_steps {
                    if cancel_token.is_cancelled() {
                        status_tx.send(DaemonStatus::Ready).await.ok();
                        return Err(anyhow::anyhow!("Long task '{}' was cancelled", description));
                    }

                    tokio::time::sleep(Duration::from_millis(100)).await;

                    if i % 10 == 0 {
                        // Update every second
                        let progress = (i * 100) / total_steps;
                        status_tx
                            .send(DaemonStatus::Busy(format!(
                                "{}: {}% complete",
                                description, progress
                            )))
                            .await
                            .ok();
                    }
                }

                status_tx.send(DaemonStatus::Ready).await.ok();

                let duration = start_time.elapsed();
                Ok(ProcessResult {
                    processed_bytes: duration_seconds * 1000, // Simulate bytes processed
                    files_created: vec![format!("{}_result.txt", description.replace(' ', "_"))],
                    duration_ms: duration.as_millis() as u64,
                    status: format!("Completed: {}", description),
                })
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
    let build_timestamp = build_timestamp.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    });

    println!(
        "Starting file processor daemon with ID: {} (build timestamp: {})",
        daemon_id, build_timestamp
    );

    // Simulate 2-second startup delay as per requirements
    println!("Initializing file processor subsystems...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Loading file type handlers...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Setting up compression engines...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Preparing validation systems...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create and start the daemon server
    let daemon = FileProcessorDaemon::new();
    let server = DaemonServer::new(daemon_id, build_timestamp, daemon);

    println!("File processor daemon ready!");

    // Run the socket server (this will block until shutdown)
    server.spawn_with_socket().await?;

    Ok(())
}
