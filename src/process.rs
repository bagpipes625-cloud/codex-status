//! Isolated Windows child process spawning (`CREATE_SUSPENDED` + JobObject) and bounded capture.

use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

const READER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IsolationPolicy {
    pub active_process_limit: Option<u32>,
}

impl IsolationPolicy {
    pub const PROCESS_TREE: Self = Self { active_process_limit: None };
    pub const SINGLE_PROCESS: Self = Self { active_process_limit: Some(1) };
}

#[derive(Debug)]
pub enum ProcessError {
    Spawn(std::io::Error),
    Isolation(String),
    Timeout,
    Wait(std::io::Error),
}

pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub struct IsolatedChild {
    pub child: Child,
    job: Option<JobGuard>,
}

impl IsolatedChild {
    /// Spawns `cmd` suspended with `CREATE_NO_WINDOW`, assigns it to a kill-on-close JobObject
    /// according to `policy`, and then resumes the primary thread.
    pub fn spawn(cmd: &mut Command, policy: IsolationPolicy) -> Result<Self, ProcessError> {
        cmd.creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
        let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;
        let suspended_thread = match SuspendedThread::find(child.id()) {
            Ok(thread) => thread,
            Err(error) => {
                terminate_child(&mut child);
                return Err(ProcessError::Isolation(error.to_string()));
            }
        };
        let job = match JobGuard::assign(&child, policy) {
            Ok(job) => job,
            Err(error) => {
                terminate_child(&mut child);
                return Err(ProcessError::Isolation(error.to_string()));
            }
        };
        if let Err(error) = suspended_thread.resume() {
            drop(job);
            terminate_child(&mut child);
            return Err(ProcessError::Isolation(error.to_string()));
        }
        drop(suspended_thread);
        Ok(Self { child, job: Some(job) })
    }

    /// Closes the JobObject first to terminate the entire process tree (releasing any inherited
    /// pipes), then ensures the direct child is terminated and reaped.
    pub fn terminate(&mut self) {
        self.job = None;
        terminate_child(&mut self.child);
    }
}

impl Drop for IsolatedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Runs `cmd` under `policy` with null stdin, bounded stdout/stderr capture, and a hard timeout.
///
/// Captures up to `max_stdout + 1` bytes from stdout so callers can detect oversized output.
pub fn run_captured(
    cmd: &mut Command,
    policy: IsolationPolicy,
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
) -> Result<CommandOutput, ProcessError> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut isolated = IsolatedChild::spawn(cmd, policy)?;

    let (stdout_tx, stdout_rx) = mpsc::sync_channel(1);
    let stdout_handle = isolated.child.stdout.take().map(|mut pipe| {
        std::thread::Builder::new()
            .name("codex-status-proc-stdout".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut buf = Vec::new();
                let _ = (&mut pipe).take(max_stdout as u64 + 1).read_to_end(&mut buf);
                let _ = stdout_tx.send(buf);
            })
            .ok()
    });
    let (stderr_tx, stderr_rx) = mpsc::sync_channel(1);
    let stderr_handle = isolated.child.stderr.take().map(|mut pipe| {
        std::thread::Builder::new()
            .name("codex-status-proc-stderr".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut buf = Vec::new();
                let _ = (&mut pipe).take(max_stderr as u64).read_to_end(&mut buf);
                let _ = stderr_tx.send(buf);
            })
            .ok()
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match isolated.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break Err(ProcessError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => break Err(ProcessError::Wait(error)),
        }
    };

    // Close the job first so any descendant that inherited stdout/stderr is terminated before
    // boundedly reclaiming the reader threads.
    isolated.terminate();
    let stdout = stdout_rx.recv_timeout(READER_CLEANUP_TIMEOUT).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(READER_CLEANUP_TIMEOUT).unwrap_or_default();
    drop(stdout_handle);
    drop(stderr_handle);

    let status = status?;
    Ok(CommandOutput { status, stdout, stderr })
}

pub fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

struct SuspendedThread(HANDLE);

impl SuspendedThread {
    fn find(process_id: u32) -> windows::core::Result<Self> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let result = (|| {
                Thread32First(snapshot, &mut entry)?;
                loop {
                    if entry.th32OwnerProcessID == process_id {
                        return OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                            .map(Self);
                    }
                    if Thread32Next(snapshot, &mut entry).is_err() {
                        return Err(windows::core::Error::from_thread());
                    }
                }
            })();
            let _ = CloseHandle(snapshot);
            result
        }
    }

    fn resume(&self) -> windows::core::Result<()> {
        let previous_count = unsafe { ResumeThread(self.0) };
        if previous_count == u32::MAX { Err(windows::core::Error::from_thread()) } else { Ok(()) }
    }
}

impl Drop for SuspendedThread {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct JobGuard(HANDLE);

impl JobGuard {
    fn assign(child: &Child, policy: IsolationPolicy) -> windows::core::Result<Self> {
        unsafe {
            let job = CreateJobObjectW(None, None)?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Some(limit) = policy.active_process_limit {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                limits.BasicLimitInformation.ActiveProcessLimit = limit;
            }
            if let Err(error) = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) {
                let _ = CloseHandle(job);
                return Err(error);
            }
            let process = HANDLE(child.as_raw_handle());
            if let Err(error) = AssignProcessToJobObject(job, process) {
                let _ = CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job))
        }
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_captured_executes_isolated_command_and_caps_stdout() {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/D", "/C", "echo 0123456789"]);
        let output =
            run_captured(&mut cmd, IsolationPolicy::SINGLE_PROCESS, Duration::from_secs(5), 4, 256)
                .unwrap();
        assert!(output.status.success());
        // Captures up to max_stdout + 1 so callers can detect oversized output.
        assert_eq!(output.stdout.len(), 5);
        assert_eq!(&output.stdout[..4], b"0123");
    }

    #[test]
    fn run_captured_enforces_timeout_and_terminates_process_tree() {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/D", "/C", "ping -n 10 127.0.0.1 >nul"]);
        let start = Instant::now();
        let result = run_captured(
            &mut cmd,
            IsolationPolicy::PROCESS_TREE,
            Duration::from_millis(200),
            1024,
            1024,
        );
        assert!(matches!(result, Err(ProcessError::Timeout)));
        assert!(start.elapsed() < Duration::from_secs(4));
    }
}
