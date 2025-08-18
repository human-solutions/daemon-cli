use anyhow::bail;
use daemon_rpc::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::time::sleep;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TestMethod {
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
    QuickTask {
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessOptions {
    pub compress: bool,
    pub validate: bool,
    pub backup: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TestResult {
    pub processed_bytes: u64,
    pub files_created: Vec<String>,
    pub duration_ms: u64,
    pub status: String,
}

impl RpcMethod for TestMethod {
    type Response = TestResult;
}

#[derive(Clone)]
pub struct DemoDaemon {
    startup_time: Instant,
    rich_mode: bool, // true for file_processor mode, false for test mode
}

impl DemoDaemon {
    pub fn new(rich_mode: bool) -> Self {
        Self {
            startup_time: Instant::now(),
            rich_mode,
        }
    }

    pub async fn simulate_startup_delay(&self) {
        if self.rich_mode {
            // Rich mode: 2-second startup with progress messages
            println!("Initializing daemon subsystems...");
            sleep(Duration::from_millis(500)).await;

            println!("Loading handlers...");
            sleep(Duration::from_millis(500)).await;

            println!("Setting up engines...");
            sleep(Duration::from_millis(500)).await;

            println!("Preparing systems...");
            sleep(Duration::from_millis(500)).await;

            println!("Daemon ready!");
        }
        // Test mode: no startup delay
    }
}

#[async_trait]
impl RpcHandler<TestMethod> for DemoDaemon {
    async fn handle(
        &mut self,
        method: TestMethod,
        cancel_token: CancellationToken,
        status_tx: tokio::sync::mpsc::Sender<DaemonStatus>,
    ) -> Result<TestResult> {
        match method {
            TestMethod::ProcessFile { path, options } => {
                let start_time = Instant::now();

                status_tx
                    .send(DaemonStatus::Busy(format!(
                        "Starting processing of {:?}",
                        path
                    )))
                    .await
                    .ok();

                // Simulate processing based on options and mode
                let steps = if self.rich_mode { 50 } else { 5 };
                for i in 0..steps {
                    if cancel_token.is_cancelled() {
                        status_tx.send(DaemonStatus::Ready).await.ok();
                        bail!("File processing cancelled");
                    }

                    sleep(Duration::from_millis(if self.rich_mode { 100 } else { 10 })).await;

                    if i % (steps / 5) == 0 {
                        let progress = (i * 100) / steps;
                        let phase = if options.compress && i < steps / 2 {
                            "compressing"
                        } else if options.validate {
                            "validating"
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

                let mut files_created = vec![format!("{}.processed", path.display())];
                if options.compress {
                    files_created.push(format!("{}.gz", path.display()));
                }
                if options.backup {
                    files_created.push(format!("{}.backup", path.display()));
                }

                Ok(TestResult {
                    processed_bytes: if self.rich_mode {
                        1024 * 1024 * 5
                    } else {
                        1024
                    },
                    files_created,
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    status: "Successfully processed file".to_string(),
                })
            }

            TestMethod::GetStatus => Ok(TestResult {
                processed_bytes: 0,
                files_created: vec![],
                duration_ms: 0,
                status: "Daemon ready for processing".to_string(),
            }),

            TestMethod::GetUptime => {
                let uptime = self.startup_time.elapsed();
                Ok(TestResult {
                    processed_bytes: 0,
                    files_created: vec![],
                    duration_ms: uptime.as_millis() as u64,
                    status: format!("Daemon uptime: {:.2}s", uptime.as_secs_f64()),
                })
            }

            TestMethod::LongTask {
                duration_seconds,
                description,
            } => {
                let start_time = Instant::now();

                status_tx
                    .send(DaemonStatus::Busy(format!("Starting: {}", description)))
                    .await
                    .ok();

                let total_steps = duration_seconds * 10;
                for i in 0..total_steps {
                    if cancel_token.is_cancelled() {
                        status_tx.send(DaemonStatus::Ready).await.ok();
                        bail!("Long task '{}' was cancelled", description);
                    }

                    sleep(Duration::from_millis(100)).await;

                    if i % 10 == 0 {
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

                Ok(TestResult {
                    processed_bytes: duration_seconds * 1000,
                    files_created: vec![format!("{}_result.txt", description.replace(' ', "_"))],
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    status: format!("Completed: {}", description),
                })
            }

            TestMethod::QuickTask { message } => {
                status_tx
                    .send(DaemonStatus::Busy(format!("Quick task: {}", message)))
                    .await
                    .ok();

                sleep(Duration::from_millis(100)).await;

                status_tx.send(DaemonStatus::Ready).await.ok();

                Ok(TestResult {
                    processed_bytes: 100,
                    files_created: vec![],
                    duration_ms: 100,
                    status: format!("Quick task completed: {}", message),
                })
            }
        }
    }
}

pub fn get_daemon_path() -> PathBuf {
    let mut exe_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    exe_path.push("target");
    exe_path.push("debug");
    exe_path.push("examples");
    exe_path.push("all_in_one");

    if cfg!(windows) {
        exe_path.set_extension("exe");
    }

    exe_path
}

pub fn get_build_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn parse_daemon_args() -> Result<(u64, u64, bool)> {
    let args: Vec<String> = env::args().collect();
    let mut daemon_id = None;
    let mut build_timestamp = None;
    let mut rich_mode = false;

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
                    bail!("--daemon-id requires a value");
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
                    bail!("--build-timestamp requires a value");
                }
            }
            "--rich" => {
                rich_mode = true;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    let daemon_id = daemon_id.ok_or_else(|| anyhow::anyhow!("--daemon-id is required"))?;
    let build_timestamp = build_timestamp.unwrap_or_else(get_build_timestamp);

    Ok((daemon_id, build_timestamp, rich_mode))
}

// Note: This is a module file, not a binary - used by all_in_one.rs
