//! Platform-specific process management utilities.

use anyhow::Result;
#[cfg(unix)]
use anyhow::bail;
#[cfg(unix)]
use tokio::time::{Duration, sleep};

/// Result of process termination attempt.
#[derive(Debug)]
pub enum TerminateResult {
    /// Process was terminated successfully.
    Terminated,
    /// Process was already dead.
    AlreadyDead,
    /// Permission denied to signal/terminate process.
    PermissionDenied,
    /// Other error occurred.
    Error(String),
}

/// Check if a process with the given PID exists.
pub fn process_exists(pid: u32) -> Result<bool> {
    platform::process_exists(pid)
}

/// Terminate a process, attempting graceful shutdown first on Unix.
///
/// # Platform Behavior
///
/// - **Unix**: Sends SIGTERM for graceful shutdown, waits up to `graceful_timeout_ms`,
///   then sends SIGKILL if still running.
/// - **Windows**: Immediately terminates the process. Windows has no SIGTERM equivalent
///   for console applications, so graceful shutdown is not possible via this method.
pub async fn terminate_process(pid: u32, graceful_timeout_ms: u64) -> TerminateResult {
    platform::terminate_process(pid, graceful_timeout_ms).await
}

/// Quick kill without graceful shutdown attempt.
///
/// Uses platform-specific kill commands:
/// - Unix: `kill` command
/// - Windows: `taskkill /F`
pub async fn kill_process(pid: u32) {
    platform::kill_process(pid).await
}

#[cfg(unix)]
mod platform {
    use super::*;
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    pub fn process_exists(pid: u32) -> Result<bool> {
        match kill(Pid::from_raw(pid as i32), None) {
            Ok(_) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(Errno::EPERM) => Ok(true), // Exists but we can't signal it
            Err(e) => bail!("Error checking process {}: {}", pid, e),
        }
    }

    pub async fn terminate_process(pid: u32, graceful_timeout_ms: u64) -> TerminateResult {
        let nix_pid = Pid::from_raw(pid as i32);

        // Verify process exists first
        match kill(nix_pid, None) {
            Ok(_) => {}
            Err(Errno::ESRCH) => return TerminateResult::AlreadyDead,
            Err(Errno::EPERM) => return TerminateResult::PermissionDenied,
            Err(e) => return TerminateResult::Error(format!("Failed to check process: {}", e)),
        }

        // Send SIGTERM for graceful shutdown
        if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
            match e {
                Errno::ESRCH => return TerminateResult::AlreadyDead,
                Errno::EPERM => return TerminateResult::PermissionDenied,
                _ => return TerminateResult::Error(format!("SIGTERM failed: {}", e)),
            }
        }

        // Wait for graceful shutdown (ceiling division ensures at least 1 iteration for any positive timeout)
        let iterations = (graceful_timeout_ms + 99) / 100;
        for _ in 0..iterations {
            sleep(Duration::from_millis(100)).await;
            match kill(nix_pid, None) {
                Err(Errno::ESRCH) => return TerminateResult::Terminated,
                Ok(_) => continue,
                Err(Errno::EPERM) => return TerminateResult::PermissionDenied,
                Err(e) => return TerminateResult::Error(format!("Check failed: {}", e)),
            }
        }

        // Still running, send SIGKILL
        if let Err(e) = kill(nix_pid, Signal::SIGKILL) {
            match e {
                Errno::ESRCH => return TerminateResult::Terminated,
                Errno::EPERM => return TerminateResult::PermissionDenied,
                _ => return TerminateResult::Error(format!("SIGKILL failed: {}", e)),
            }
        }

        // Wait for SIGKILL to take effect
        sleep(Duration::from_millis(500)).await;

        match kill(nix_pid, None) {
            Err(Errno::ESRCH) => TerminateResult::Terminated,
            Ok(_) => TerminateResult::Error("Process survived SIGKILL".to_string()),
            Err(e) => TerminateResult::Error(format!("Final check failed: {}", e)),
        }
    }

    pub async fn kill_process(pid: u32) {
        use tokio::process::Command;
        let _ = Command::new("kill").arg(pid.to_string()).output().await;
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        TerminateProcess as WinTerminateProcess, WaitForSingleObject,
    };

    pub fn process_exists(pid: u32) -> Result<bool> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };

        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Ok(false)
        } else {
            unsafe { CloseHandle(handle) };
            Ok(true)
        }
    }

    pub async fn terminate_process(pid: u32, _graceful_timeout_ms: u64) -> TerminateResult {
        // Windows has no SIGTERM equivalent for console applications.
        // TerminateProcess is immediate (like SIGKILL).

        let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };

        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            // Check if process exists with limited permissions
            let check = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if check.is_null() || check == INVALID_HANDLE_VALUE {
                return TerminateResult::AlreadyDead;
            }
            unsafe { CloseHandle(check) };
            return TerminateResult::PermissionDenied;
        }

        let result = unsafe { WinTerminateProcess(handle, 1) };
        if result == 0 {
            unsafe { CloseHandle(handle) };
            return TerminateResult::Error("TerminateProcess failed".to_string());
        }

        // Wait for process to exit (up to 5 seconds)
        let wait = unsafe { WaitForSingleObject(handle, 5000) };
        unsafe { CloseHandle(handle) };

        if wait == WAIT_OBJECT_0 {
            TerminateResult::Terminated
        } else {
            TerminateResult::Error("Process did not terminate".to_string())
        }
    }

    pub async fn kill_process(pid: u32) {
        use tokio::process::Command;
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_current_process_exists() {
        let pid = std::process::id();
        assert!(process_exists(pid).unwrap());
    }

    #[tokio::test]
    async fn test_nonexistent_process() {
        // Very high PID unlikely to exist
        let result = process_exists(4_000_000_000);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_terminate_nonexistent_process() {
        // Terminating a nonexistent process should return AlreadyDead
        let result = terminate_process(4_000_000_000, 100).await;
        assert!(
            matches!(result, TerminateResult::AlreadyDead),
            "Expected AlreadyDead for nonexistent process"
        );
    }

    #[tokio::test]
    async fn test_kill_nonexistent_process() {
        // Kill on nonexistent process should not panic
        kill_process(4_000_000_000).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_terminate_spawned_process() {
        use tokio::process::Command;

        // Spawn a simple sleep process
        let mut child = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("Failed to spawn sleep process");

        let pid = child.id().expect("Process should have PID");

        // Verify process exists
        assert!(process_exists(pid).unwrap(), "Spawned process should exist");

        // Terminate it with graceful timeout
        let result = terminate_process(pid, 500).await;

        // Note: When terminating a direct child process, it becomes a zombie until
        // the parent (us) waits on it. The terminate_process function may report
        // "Process survived SIGKILL" because zombies still appear in the process table.
        // This is expected behavior - in production, daemon processes are detached
        // and reaped by init/systemd, not our process.
        //
        // Accept either Terminated or the "zombie" error case
        assert!(
            matches!(
                result,
                TerminateResult::Terminated | TerminateResult::Error(_)
            ),
            "Process should be killed, got {:?}",
            result
        );

        // Reap the zombie process by waiting on the child
        let exit_status = child.wait().await;
        assert!(
            exit_status.is_ok(),
            "Should be able to wait on terminated child"
        );

        // Verify process no longer exists (after reaping)
        assert!(
            !process_exists(pid).unwrap(),
            "Process should not exist after reaping"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_sigterm_then_sigkill_escalation() {
        use std::process::Stdio;
        use tokio::process::Command;

        // Spawn a process that traps SIGTERM and ignores it
        // Using bash with a trap to ignore SIGTERM
        let mut child = Command::new("bash")
            .arg("-c")
            .arg("trap '' TERM; sleep 60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to spawn bash process");

        let pid = child.id().expect("Process should have PID");

        // Give the trap time to be set up
        sleep(Duration::from_millis(100)).await;

        // Verify process exists
        assert!(process_exists(pid).unwrap(), "Spawned process should exist");

        // Terminate with very short graceful timeout (100ms)
        // This should escalate to SIGKILL since SIGTERM is trapped
        let result = terminate_process(pid, 100).await;

        // Note: Same zombie issue as above - the process may appear to survive
        // because it's a zombie until we reap it.
        assert!(
            matches!(
                result,
                TerminateResult::Terminated | TerminateResult::Error(_)
            ),
            "Process should be killed (possibly as zombie), got {:?}",
            result
        );

        // Reap the zombie process by waiting on the child
        let exit_status = child.wait().await;
        assert!(
            exit_status.is_ok(),
            "Should be able to wait on terminated child"
        );

        // Verify process no longer exists (after reaping)
        assert!(
            !process_exists(pid).unwrap(),
            "Process should not exist after reaping"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_terminate_spawned_process_windows() {
        use std::process::Stdio;
        use tokio::process::Command;

        // Spawn a long-running process on Windows
        let child = Command::new("timeout")
            .args(["/t", "60", "/nobreak"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to spawn timeout process");

        let pid = child.id().expect("Process should have PID");

        // Verify process exists
        assert!(process_exists(pid).unwrap(), "Spawned process should exist");

        // Terminate it (Windows has no graceful shutdown, just immediate termination)
        let result = terminate_process(pid, 500).await;

        assert!(
            matches!(result, TerminateResult::Terminated),
            "Process should terminate successfully, got {:?}",
            result
        );

        // Verify process no longer exists
        assert!(
            !process_exists(pid).unwrap(),
            "Process should not exist after termination"
        );
    }
}
