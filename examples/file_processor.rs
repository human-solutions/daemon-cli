use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use daemon_rpc::prelude::*;
use std::io::{self, Write};
use std::time::Duration;

// Import the daemon's types
use std::path::PathBuf;

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

struct App {
    client: DaemonClient<FileMethod>,
    current_status: String,
    last_result: Option<ProcessResult>,
    running_task: bool,
    should_exit: bool,
}

impl App {
    async fn new() -> Result<Self> {
        // Get the file processor daemon binary path
        let daemon_executable = get_daemon_path();
        let build_timestamp = get_build_timestamp();

        print!("🔄 Connecting to file processor daemon...");
        io::stdout().flush().ok();

        // This will auto-spawn the daemon and show the 2s startup delay
        let mut client = DaemonClient::connect(1000, daemon_executable, build_timestamp).await?;

        println!(" ✓");

        // Wait a moment to see initial status
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Check initial status
        let status_result = client.request(FileMethod::GetStatus).await?;
        let initial_status = match status_result {
            RpcResponse::Success { output } => output.status,
            _ => "Unknown".to_string(),
        };

        Ok(Self {
            client,
            current_status: initial_status,
            last_result: None,
            running_task: false,
            should_exit: false,
        })
    }

    async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;

        // Status monitoring task
        let status_task = {
            let mut status_receiver = self.client.status_receiver.resubscribe();
            tokio::spawn(async move {
                let mut last_status = None;
                while let Ok(status) = status_receiver.recv().await {
                    if Some(&status) != last_status.as_ref() {
                        match &status {
                            DaemonStatus::Ready => {
                                App::print_status_line("✅ Ready");
                            }
                            DaemonStatus::Busy(msg) => {
                                App::print_status_line(&format!("🔄 {}", msg));
                            }
                            DaemonStatus::Error(err) => {
                                App::print_status_line(&format!("❌ Error: {}", err));
                            }
                        }
                        last_status = Some(status);
                    }
                }
            })
        };

        self.draw_ui().await?;

        // Main event loop
        loop {
            if self.should_exit {
                break;
            }

            // Handle keyboard input with timeout
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key_event) = event::read()? {
                    self.handle_key_event(key_event).await?;
                }
            }

            self.draw_ui().await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Cleanup
        status_task.abort();
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;

        // Note: We intentionally don't call client.shutdown() to leave daemon running

        Ok(())
    }

    async fn handle_key_event(&mut self, key_event: KeyEvent) -> Result<()> {
        match key_event.code {
            KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_exit = true;
            }
            KeyCode::Esc => {
                if self.running_task {
                    App::print_status_line("🛑 Cancelling current task...");
                    match self.client.cancel_current_task().await {
                        Ok(true) => {
                            App::print_status_line("✅ Task cancelled successfully");
                            self.running_task = false;
                        }
                        Ok(false) => {
                            App::print_status_line("⚠️  No task running or cancellation failed");
                        }
                        Err(e) => {
                            App::print_status_line(&format!("❌ Cancel error: {}", e));
                        }
                    }
                }
            }
            KeyCode::Char('1') => {
                if !self.running_task {
                    self.start_file_processing().await?;
                }
            }
            KeyCode::Char('2') => {
                if !self.running_task {
                    self.start_long_task().await?;
                }
            }
            KeyCode::Char('3') => {
                if !self.running_task {
                    self.get_uptime().await?;
                }
            }
            KeyCode::Char('q') => {
                self.should_exit = true;
            }
            _ => {}
        }
        Ok(())
    }

    async fn start_file_processing(&mut self) -> Result<()> {
        self.running_task = true;
        App::print_status_line("🚀 Starting file processing...");

        let options = ProcessOptions {
            compress: true,
            validate: true,
            backup: false,
        };

        let result = self
            .client
            .request(FileMethod::ProcessFile {
                path: "/tmp/large_file.txt".into(),
                options,
            })
            .await;

        self.running_task = false;

        match result? {
            RpcResponse::Success { output } => {
                self.last_result = Some(output);
                App::print_status_line("✅ File processing completed!");
            }
            RpcResponse::Error { error } => {
                App::print_status_line(&format!("❌ Processing failed: {}", error));
            }
            _ => {
                App::print_status_line("⚠️  Unexpected response type");
            }
        }

        Ok(())
    }

    async fn start_long_task(&mut self) -> Result<()> {
        self.running_task = true;
        App::print_status_line("🚀 Starting long background task...");

        let result = self
            .client
            .request(FileMethod::LongTask {
                duration_seconds: 10,
                description: "Processing large dataset".to_string(),
            })
            .await;

        self.running_task = false;

        match result? {
            RpcResponse::Success { output } => {
                self.last_result = Some(output);
                App::print_status_line("✅ Long task completed!");
            }
            RpcResponse::Error { error } => {
                App::print_status_line(&format!("❌ Task failed: {}", error));
            }
            _ => {
                App::print_status_line("⚠️  Unexpected response type");
            }
        }

        Ok(())
    }

    async fn get_uptime(&mut self) -> Result<()> {
        let result = self.client.request(FileMethod::GetUptime).await;

        match result? {
            RpcResponse::Success { output } => {
                App::print_status_line(&format!("📊 {}", output.status));
            }
            RpcResponse::Error { error } => {
                App::print_status_line(&format!("❌ Uptime check failed: {}", error));
            }
            _ => {
                App::print_status_line("⚠️  Unexpected response type");
            }
        }

        Ok(())
    }

    async fn draw_ui(&self) -> Result<()> {
        execute!(
            io::stdout(),
            cursor::MoveTo(0, 0),
            Print(
                "┌─────────────────────────────────────────────────────────────────────────┐\r\n"
            ),
            Print("│                       📁 File Processor CLI                            │\r\n"),
            Print(
                "├─────────────────────────────────────────────────────────────────────────┤\r\n"
            ),
            Print(
                "│                                                                         │\r\n"
            ),
        )?;

        // Status line
        execute!(
            io::stdout(),
            cursor::MoveTo(2, 4),
            SetForegroundColor(Color::Cyan),
            Print("Status: "),
            ResetColor,
            Print(&self.current_status),
        )?;

        // Commands
        execute!(
            io::stdout(),
            cursor::MoveTo(0, 6),
            Print(
                "├─────────────────────────────────────────────────────────────────────────┤\r\n"
            ),
            Print(
                "│ Commands:                                                               │\r\n"
            ),
            Print("│   [1] Process file (with compression & validation)                     │\r\n"),
            Print("│   [2] Long-running task (10 seconds)                                   │\r\n"),
            Print(
                "│   [3] Get daemon uptime                                                 │\r\n"
            ),
            Print(
                "│                                                                         │\r\n"
            ),
            Print(
                "│ Controls:                                                               │\r\n"
            ),
            Print(
                "│   [ESC] Cancel current task                                             │\r\n"
            ),
            Print("│   [Q] Quit CLI (daemon keeps running)                                  │\r\n"),
            Print("│   [Ctrl+C] Force exit                                                  │\r\n"),
        )?;

        // Results section
        execute!(
            io::stdout(),
            cursor::MoveTo(0, 16),
            Print(
                "├─────────────────────────────────────────────────────────────────────────┤\r\n"
            ),
            Print(
                "│ Last Result:                                                            │\r\n"
            ),
        )?;

        if let Some(result) = &self.last_result {
            execute!(
                io::stdout(),
                cursor::MoveTo(2, 18),
                Print(&format!("Files: {:?}", result.files_created)),
                cursor::MoveTo(2, 19),
                Print(&format!(
                    "Bytes: {} | Duration: {}ms",
                    result.processed_bytes, result.duration_ms
                )),
                cursor::MoveTo(2, 20),
                Print(&format!("Status: {}", result.status)),
            )?;
        } else {
            execute!(
                io::stdout(),
                cursor::MoveTo(2, 18),
                SetForegroundColor(Color::DarkGrey),
                Print("No operations completed yet"),
                ResetColor,
            )?;
        }

        execute!(
            io::stdout(),
            cursor::MoveTo(0, 22),
            Print(
                "└─────────────────────────────────────────────────────────────────────────┘\r\n"
            ),
        )?;

        if self.running_task {
            execute!(
                io::stdout(),
                cursor::MoveTo(0, 24),
                SetForegroundColor(Color::Yellow),
                Print("⚠️  Task running... Press [ESC] to cancel"),
                ResetColor,
            )?;
        }

        io::stdout().flush()?;
        Ok(())
    }

    fn print_status_line(message: &str) {
        // For status updates from background task
        // In a real implementation, this would coordinate with the main UI
        eprintln!("[Status] {}", message);
    }
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
    // In a real application, this would come from build.rs
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
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
            println!("✅ Daemon build successful");
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

    // Create and run the app
    let mut app = App::new().await.map_err(|e| {
        eprintln!("❌ Failed to connect to daemon: {}", e);
        e
    })?;

    println!("🎉 File Processor CLI ready! Starting interactive mode...");
    tokio::time::sleep(Duration::from_millis(1000)).await;

    if let Err(e) = app.run().await {
        disable_raw_mode().ok();
        execute!(io::stdout(), LeaveAlternateScreen).ok();
        eprintln!("❌ Application error: {}", e);
        return Err(e);
    }

    println!("👋 CLI exited. Daemon continues running for other clients.");
    Ok(())
}
