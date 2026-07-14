//! Registry of child processes started by the code-gen "run" action
//! (`POST /api/generate/cargo` with `action = "run"`).
//!
//! Exists so process termination and liveness checks use the
//! cross-platform [`std::process::Child`] API (`kill()` / `try_wait()`)
//! instead of shelling out to the Unix-only `kill`/`pkill` binaries.
//! That makes the code-gen run/stop feature work on Windows as well as
//! macOS/Linux. See `handlers::generate::{run_cargo, stop_cargo}`.

use std::collections::HashMap;
use std::process::Child;

struct Entry {
    working_dir: String,
    child: Child,
}

/// Tracks child processes keyed by PID so `stop_cargo(pid)` can find its
/// process directly. Each entry remembers the `working_dir` so a fresh
/// "run" for the same directory can terminate the previous process —
/// the portable replacement for the old `pkill -f "target/debug/.*<dir>"`.
#[derive(Default)]
pub struct ProcRegistry {
    running: HashMap<u32, Entry>,
}

impl ProcRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a freshly spawned child under its PID.
    pub fn insert(&mut self, working_dir: String, child: Child) {
        self.running.insert(child.id(), Entry { working_dir, child });
    }

    /// True if the tracked child for `pid` is still alive. Reaps and drops
    /// the entry if the process has exited. Replaces `kill -0 <pid>`.
    pub fn is_running(&mut self, pid: u32) -> bool {
        let Some(entry) = self.running.get_mut(&pid) else {
            return false;
        };
        match entry.child.try_wait() {
            // Still running — no exit status yet.
            Ok(None) => true,
            // Exited (try_wait reaped it) or the wait failed; forget it.
            Ok(Some(_)) | Err(_) => {
                self.running.remove(&pid);
                false
            }
        }
    }

    /// Kill the tracked process for `pid`, reaping it. Returns true if the
    /// process was tracked. Replaces `kill <pid>`.
    pub fn kill(&mut self, pid: u32) -> bool {
        match self.running.remove(&pid) {
            Some(mut entry) => {
                // `kill()` errors if the process already exited — that's the
                // outcome we want, so ignore it. `wait()` reaps the child so
                // it doesn't linger as a zombie on Unix.
                let _ = entry.child.kill();
                let _ = entry.child.wait();
                true
            }
            None => false,
        }
    }

    /// Kill and forget every tracked process whose `working_dir` matches.
    /// Returns how many were terminated. Replaces `pkill -f`.
    pub fn kill_matching_dir(&mut self, working_dir: &str) -> usize {
        let pids: Vec<u32> = self
            .running
            .iter()
            .filter(|(_, e)| e.working_dir == working_dir)
            .map(|(pid, _)| *pid)
            .collect();
        let mut killed = 0;
        for pid in pids {
            if self.kill(pid) {
                killed += 1;
            }
        }
        killed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A child that stays alive long enough to observe, on any platform.
    fn spawn_sleeper() -> Child {
        #[cfg(windows)]
        {
            Command::new("cmd")
                .args(["/C", "ping -n 30 127.0.0.1 > NUL"])
                .spawn()
                .expect("spawn sleeper")
        }
        #[cfg(not(windows))]
        {
            Command::new("sleep").arg("30").spawn().expect("spawn sleeper")
        }
    }

    /// A child that exits immediately, on any platform.
    fn spawn_quick() -> Child {
        #[cfg(windows)]
        {
            Command::new("cmd").args(["/C", "exit"]).spawn().expect("spawn quick")
        }
        #[cfg(not(windows))]
        {
            Command::new("true").spawn().expect("spawn quick")
        }
    }

    #[test]
    fn kill_terminates_tracked_process() {
        let mut reg = ProcRegistry::new();
        let child = spawn_sleeper();
        let pid = child.id();
        reg.insert("/work/alpha".into(), child);

        assert!(reg.is_running(pid), "sleeper should be running right after spawn");
        assert!(reg.kill(pid), "kill should report success for a tracked live process");
        assert!(!reg.is_running(pid), "process should be gone after kill");
    }

    #[test]
    fn kill_unknown_pid_returns_false() {
        let mut reg = ProcRegistry::new();
        assert!(!reg.kill(4_294_967_295), "unknown pid must not report a kill");
    }

    #[test]
    fn kill_matching_dir_terminates_only_that_dir() {
        let mut reg = ProcRegistry::new();
        let a = spawn_sleeper();
        let a_pid = a.id();
        let b = spawn_sleeper();
        let b_pid = b.id();
        reg.insert("/work/alpha".into(), a);
        reg.insert("/work/beta".into(), b);

        let killed = reg.kill_matching_dir("/work/alpha");
        assert_eq!(killed, 1, "exactly the one alpha process should be killed");
        assert!(!reg.is_running(a_pid), "alpha process should be gone");
        assert!(reg.is_running(b_pid), "beta process should be untouched");

        reg.kill(b_pid); // cleanup
    }

    #[test]
    fn is_running_false_after_process_exits() {
        let mut reg = ProcRegistry::new();
        let child = spawn_quick();
        let pid = child.id();
        reg.insert("/work/quick".into(), child);

        // Give the OS a moment to run the process to completion.
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!reg.is_running(pid), "an exited process must not report running");
    }
}
