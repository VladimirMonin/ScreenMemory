//! Owned child-process guard for the single explicitly authorized host autostart.
//! Only the process this CLI created can ever be terminated; never a foreign PID,
//! an image name, Node or LM Studio.
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const TERMINATE_BUDGET: Duration = Duration::from_secs(2);

/// A capturing parent (a shell `$(...)`, a test harness or a pipe-based wrapper) keeps
/// the CLI's standard pipe handles open until every inheritable copy is closed. A host
/// that outlives this CLI must never keep such a copy, so the three standard handles are
/// detached before the single explicit spawn; the host gets its own configured stdio.
mod inherited_pipes {
    use windows_sys::Win32::{
        Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE},
        System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        },
    };

    pub(super) fn detach_standard_handles() {
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle: HANDLE = unsafe { GetStdHandle(id as STD_HANDLE) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                continue;
            }
            // Best effort: a failure only leaves the previous inheritance behaviour.
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchError {
    SpawnFailed,
}

/// A child handle owned by this CLI from the moment `launch` returns.
pub trait LaunchedHost: Send {
    fn has_exited(&mut self) -> bool;
    /// The authenticated session is up: the host now outlives this CLI process.
    fn mark_established(&mut self);
    /// Terminate exactly this owned child handle and wait a bounded time for its exit.
    fn terminate_owned(&mut self);
}

pub trait HostLauncher: Send + Sync {
    fn launch(&self, executable: &Path, root: &Path) -> Result<Box<dyn LaunchedHost>, LaunchError>;
}

/// Fixed trusted executable, absolute root argument, no shell, no PATH lookup.
pub struct ProcessLauncher;
impl HostLauncher for ProcessLauncher {
    fn launch(&self, executable: &Path, root: &Path) -> Result<Box<dyn LaunchedHost>, LaunchError> {
        // The host outlives this CLI process, so it must never inherit the caller's
        // stderr pipe (a capturing wrapper would wait for a handle that stays open).
        // Safe pre-root host diagnostics land in the owned root's cache directory and
        // the file is truncated per launch, so repeated attempts stay bounded.
        let diagnostics = root.join("cache").join("host-stderr.log");
        let stderr = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&diagnostics)
        {
            Ok(file) => Stdio::from(file),
            Err(_) => Stdio::null(),
        };
        inherited_pipes::detach_standard_handles();
        let child = Command::new(executable)
            .arg("--root")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .map_err(|_| LaunchError::SpawnFailed)?;
        Ok(Box::new(OwnedChild {
            child,
            established: false,
        }))
    }
}

struct OwnedChild {
    child: Child,
    established: bool,
}
impl LaunchedHost for OwnedChild {
    fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
    fn mark_established(&mut self) {
        self.established = true;
    }
    fn terminate_owned(&mut self) {
        if self.has_exited() {
            return;
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + TERMINATE_BUDGET;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // A host this CLI started but never reached is not left holding the lease;
        // an established host is left running (a CLI exit never shuts the host down).
        if !self.established {
            self.terminate_owned();
        }
    }
}
