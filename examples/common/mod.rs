use anyhow::Result;
use daemon_cli::prelude::*;
use std::time::Duration;
use std::{
    env,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    time::sleep,
};

#[derive(Clone)]
pub struct CommandProcessor {
    startup_time: Instant,
}

impl CommandProcessor {
    pub fn new() -> Self {
        Self {
            startup_time: Instant::now(),
        }
    }
}

#[async_trait]
impl CommandHandler for CommandProcessor {
    async fn handle(
        &self,
        command: &str,
        mut output: impl AsyncWrite + Send + Unpin,
        cancel_token: CancellationToken,
    ) -> Result<()> {
        let parts: Vec<&str> = command.trim().split_whitespace().collect();

        match parts.get(0) {
            Some(&"status") => {
                output.write_all(b"Daemon ready for processing\n").await?;
                Ok(())
            }

            Some(&"uptime") => {
                let uptime = self.startup_time.elapsed();
                let message = format!("Daemon uptime: {:.2}s\n", uptime.as_secs_f64());
                output.write_all(message.as_bytes()).await?;
                Ok(())
            }

            Some(&"process") => {
                let filename = parts.get(1).unwrap_or(&"file.txt");

                output
                    .write_all(format!("Processing: {}\n", filename).as_bytes())
                    .await?;

                // Simulate processing with progress updates
                for i in 0..10 {
                    if cancel_token.is_cancelled() {
                        output.write_all(b"Processing cancelled\n").await?;
                        return Err(anyhow::anyhow!("Processing cancelled"));
                    }

                    sleep(Duration::from_millis(200)).await;

                    let progress = (i + 1) * 10;
                    output
                        .write_all(format!("Progress: {}%\n", progress).as_bytes())
                        .await?;
                }

                output
                    .write_all(format!("Successfully processed {}\n", filename).as_bytes())
                    .await?;
                Ok(())
            }

            Some(&"long") => {
                let duration_secs = parts
                    .get(1)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(5);

                output
                    .write_all(
                        format!("Starting long task ({} seconds)\n", duration_secs).as_bytes(),
                    )
                    .await?;

                let total_steps = duration_secs * 10;
                for i in 0..total_steps {
                    if cancel_token.is_cancelled() {
                        output.write_all(b"Task cancelled\n").await?;
                        return Err(anyhow::anyhow!("Task cancelled"));
                    }

                    sleep(Duration::from_millis(100)).await;

                    if i % 10 == 0 {
                        let progress = (i * 100) / total_steps;
                        output
                            .write_all(format!("Progress: {}%\n", progress).as_bytes())
                            .await?;
                    }
                }

                output.write_all(b"Task completed\n").await?;
                Ok(())
            }

            Some(&"echo") => {
                // Echo back the rest of the command
                let message = parts[1..].join(" ");
                output.write_all(message.as_bytes()).await?;
                output.write_all(b"\n").await?;
                Ok(())
            }

            _ => Err(anyhow::anyhow!(
                "Unknown command. Available: status, uptime, process [file], long [seconds], echo [message]"
            )),
        }
    }
}

pub fn get_daemon_path() -> PathBuf {
    let mut exe_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    exe_path.push("target");
    exe_path.push("debug");
    exe_path.push("examples");
    exe_path.push("cli");

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

pub fn parse_daemon_args() -> Result<(u64, u64)> {
    let args: Vec<String> = env::args().collect();
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
    let build_timestamp = build_timestamp.unwrap_or_else(get_build_timestamp);

    Ok((daemon_id, build_timestamp))
}
