//! Owner liveness for the single-writer guard (REQ-SW-04, REQ-SW-06).
//!
//! The question this answers: is the PROCESS that opened a RUNNING
//! execution row still alive? The answer is an OBSERVATION, never an
//! action: this module reads `/proc` and the hostname file, writes nothing
//! to the store and sends no signal to any process — the ratified boundary
//! (REQ-SW-06: «никакого автоматического убийства чужих процессов; детект и
//! эскалация, решение за человеком»). The only resolution of an orphan is a
//! human closing its row via `exec fail|cancel`.
//!
//! Liveness vocabulary is exactly the ratified OQ-3 answer:
//! * a host OTHER than the current one is UNCHECKABLE — it renders as
//!   `Unknown` («as-is», never dead);
//! * pid-reuse on the LOCAL host is closed by pid + PROCESS start time
//!   (`kill(pid,0)` alone is explicitly not enough);
//! * a row with no recorded owner (written before rung 007, or directly
//!   into the store) is `Unknown` — a triage signal, never dead.
//!
//! Non-Linux platforms: without `/proc` nothing is provable, so every
//! check degrades to `Unknown` — conservative by design (REQ-SW-04 AC: a
//! dead mark needs proof).

use std::fs;
use std::path::Path;

/// The three-state answer of a liveness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The owner process is provably alive on the current host (pid exists
    /// and, when the start time is recorded on both sides, matches).
    Alive,
    /// The owner process is provably dead: its pid is gone on this host, or
    /// the pid is present but the process start time differs — the pid was
    /// RECYCLED by a different process (the reuse defense of OQ-3).
    Dead,
    /// Not checkable: no owner recorded (pre-007 row), the owner lives on
    /// another host, or the platform offers no `/proc`. Display as-is;
    /// NEVER silently promoted to Dead.
    Unknown,
}

impl Liveness {
    pub fn as_str(self) -> &'static str {
        match self {
            Liveness::Alive => "alive",
            Liveness::Dead => "dead",
            Liveness::Unknown => "unknown",
        }
    }
}

/// The owner identity a platform-side caller stamps into a NEW execution
/// row at start (REQ-SW-03). The pid is the process executing `start` —
/// the spec's wording: «процесса, открывшего исполнение». `host` and
/// `process_start` are NULL where the OS cannot answer (non-Linux), which
/// keeps every later check `Unknown` rather than inventing a verdict.
#[derive(Debug, Clone)]
pub struct OwnerInfo {
    pub pid: i64,
    pub host: Option<String>,
    pub process_start: Option<i64>,
}

/// Observe the CURRENT process as an owner (called at execution start by
/// the CLI and the engine — platform-side, never model-side).
pub fn current_owner() -> OwnerInfo {
    let pid = std::process::id() as i64;
    OwnerInfo { pid, host: current_host(), process_start: proc_start_epoch(pid) }
}

impl OwnerInfo {
    /// Fill the three `RoleExecution` owner columns from this observation.
    pub fn into_columns(self) -> (Option<i64>, Option<String>, Option<i64>) {
        (Some(self.pid), self.host, self.process_start)
    }
}

/// Check one recorded owner against the live OS state. `Ok`/`Err` is never
/// the channel — an unanswerable question answers `Unknown`.
pub fn check_owner(pid: Option<i64>, host: Option<&str>, process_start: Option<i64>) -> Liveness {
    let pid = match pid {
        Some(p) => p,
        None => return Liveness::Unknown, // owner unknown ≠ owner dead (REQ-SW-04 AC3)
    };
    // Ratified OQ-3: liveness is checked on the LOCAL host only; any other
    // host is not observable here and must never be guessed dead.
    let Some(current_host) = current_host() else { return Liveness::Unknown };
    if host.map(|h| h != current_host.as_str()).unwrap_or(true) {
        return Liveness::Unknown;
    }
    // Remote check needs /proc: without it (non-Linux) nothing is provable.
    let proc_pid = Path::new("/proc").join(pid.to_string());
    if !Path::new("/proc").is_dir() {
        return Liveness::Unknown;
    }
    if !proc_pid.exists() {
        // /proc is live on this host and the pid is absent: the process is
        // gone. This is the ONE provable death.
        return Liveness::Dead;
    }
    // Pid present: close pid-reuse with the process start time (kill(pid,0)
    // alone would be fooled by a recycled pid — explicit OQ-3 ratification).
    match (process_start, proc_start_epoch(pid)) {
        // Tolerance absorbs a clock step (NTP/manual) between write and
        // check; both sides otherwise derive from the same boot-fixed
        // sources and agree exactly. A REUSED pid differs by far more.
        (Some(recorded), Some(now)) if (recorded - now).abs() > START_TOLERANCE_SECS => Liveness::Dead,
        _ => Liveness::Alive, // matching start, or an uncomparable clock source
    }
}

/// Tolerance for the start-time equality (see the match arm above).
const START_TOLERANCE_SECS: i64 = 60;

/// The current hostname (Linux: `/proc/sys/kernel/hostname`). `None` makes
/// every check `Unknown` — an unobservable host never decides death.
pub fn current_host() -> Option<String> {
    let raw = fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    let h = raw.trim().to_string();
    (!h.is_empty()).then_some(h)
}

/// Process start time in whole epoch seconds: `btime` (boot, epoch secs)
/// plus `starttime` (clock ticks since boot, field 22 of `/proc/<pid>/stat`)
/// divided by USER_HZ. The value is deterministic for the life of one
/// boot, so a write and a later read agree exactly — only a REUSED pid (or
/// a reboot under the pid) produces a different number. Returns `None`
/// when `/proc` cannot answer; the caller then keeps the row comparable
/// by pid alone (never declares it dead).
pub fn proc_start_epoch(pid: i64) -> Option<i64> {
    // Field 22 lives after the parenthesized comm, which may contain
    // spaces and ')': split on the LAST ')'.
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rfind(')')?;
    let fields: Vec<&str> = stat[tail + 1..].split_whitespace().collect();
    // fields[0] is field 3 (state); starttime is field 22 => index 19.
    let ticks: i64 = fields.get(19)?.parse().ok()?;
    let btime = boot_epoch_secs()?;
    Some(btime + ticks / USER_HZ)
}

/// Kernel clock ticks per second (the Linux `USER_HZ` constant, 100 on
/// every supported arch). Both write and read sides use the SAME constant
/// on the same machine, so the epoch value stays self-consistent.
const USER_HZ: i64 = 100;

fn boot_epoch_secs() -> Option<i64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    stat.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_is_alive() {
        let owner = current_owner();
        assert!(owner.pid > 0);
        let (pid, host, start) = (Some(owner.pid), owner.host.as_deref(), owner.process_start);
        assert_eq!(check_owner(pid, host, start), Liveness::Alive, "the test process is its own live owner");
    }

    #[test]
    fn absent_pid_is_dead_on_this_host() {
        // No pid near i64::MAX exists; /proc answers "not there" — the ONE
        // provable death.
        let owner = current_owner();
        assert_eq!(check_owner(Some(i64::MAX), owner.host.as_deref(), owner.process_start), Liveness::Dead);
    }

    #[test]
    fn pid_reuse_is_caught_by_the_start_time() {
        // A live pid carrying a WRONG recorded start time: `kill(pid,0)`
        // would call it alive, the start-time comparison must call it dead
        // (explicit OQ-3: pid alone is not enough).
        let owner = current_owner();
        assert!(owner.process_start.is_some(), "/proc-backed environment");
        let reused = owner.process_start.unwrap() - 3600;
        assert_eq!(check_owner(Some(owner.pid), owner.host.as_deref(), Some(reused)), Liveness::Dead);
    }

    #[test]
    fn other_host_is_unknown_never_dead() {
        let owner = current_owner();
        assert_eq!(check_owner(Some(owner.pid), Some("some-remote-host"), owner.process_start), Liveness::Unknown);
    }

    #[test]
    fn no_owner_is_unknown_never_dead() {
        assert_eq!(check_owner(None, None, None), Liveness::Unknown);
    }
}
