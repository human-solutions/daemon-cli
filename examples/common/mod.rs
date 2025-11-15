use anyhow::Result;
use daemon_cli::prelude::*;
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    time::sleep,
};

#[derive(Clone)]
#[allow(dead_code)]
pub struct CommandProcessor {
    startup_time: Instant,
}

#[allow(dead_code)]
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
    ) -> Result<i32> {
        let parts: Vec<&str> = command.trim().split_whitespace().collect();

        match parts.get(0) {
            Some(&"status") => {
                output.write_all(b"Daemon ready for processing\n").await?;
                Ok(0)
            }

            Some(&"uptime") => {
                let uptime = self.startup_time.elapsed();
                let message = format!("Daemon uptime: {:.2}s\n", uptime.as_secs_f64());
                output.write_all(message.as_bytes()).await?;
                Ok(0)
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
                Ok(0)
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
                Ok(0)
            }

            Some(&"echo") => {
                // Echo back the rest of the command
                let message = parts[1..].join(" ");
                output.write_all(message.as_bytes()).await?;
                output.write_all(b"\n").await?;
                Ok(0)
            }

            _ => {
                output.write_all(b"Unknown command. Available: status, uptime, process [file], long [seconds], echo [message]\n").await?;
                Ok(127) // Exit code 127 for unknown command
            }
        }
    }
}
