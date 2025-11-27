//! Platform-specific process management utilities.

use anyhow::{Result, bail};
use tokio::time::{sleep, Duration};

/// Result of process termination attempt.
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

        // Wait for graceful shutdown
        let iterations = graceful_timeout_ms / 100;
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
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess as WinTerminateProcess,
        WaitForSingleObject, PROCESS_TERMINATE, PROCESS_SYNCHRONIZE,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn process_exists(pid: u32) -> Result<bool> {
        let handle = unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
        };

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

        let handle = unsafe {
            OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid)
        };

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
}
