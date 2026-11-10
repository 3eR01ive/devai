//! Asynchronous role launch: the PLATFORM owns the process (REQ-AR-01/04/08).
//!
//! `devai task run` no longer executes the role inside the caller. It starts a
//! platform runner — the same binary in the hidden `__supervise` mode — in its
//! OWN session (`setsid`), with stdin closed and output redirected to the
//! home's evidence tree, then hands the caller a handle after the runner has
//! opened the execution row. Three facts follow from that arrangement:
//!
//! * the caller's death (a systemd unit ending, a script exiting, a terminal
//!   closing) does not take the role down: it is not in the caller's session or
//!   process group any more — this is the 12/23/39/59-minute incident;
//! * the RUNNER — not the command that returned — waits for the role, records
//!   its exit code and the output tail, and closes the row (REQ-AR-03);
//! * either the caller gets an execution id whose owner process is alive, or
//!   the launch is refused: there is no «started» without a process (REQ-AR-08).
//!
//! The handshake is a file the runner writes exactly once (atomically): the
//! caller polls it for a bounded time. No pipes survive the caller, so no
//! SIGPIPE can turn a launch into a phantom.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The hidden subcommand the platform runner is executed as. Not a user
/// surface: it re-opens the home and drives ONE run.
pub const RUNNER_SUBCOMMAND: &str = "__supervise";

/// How long the caller waits for the runner to report the opened execution
/// row. The row is opened before any role work starts, so this is a handshake
/// budget, not a run budget.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(60);

/// The runner's own transcript, under the run's evidence directory.
pub const SUPERVISOR_LOG: &str = "supervisor.log";

/// The settled run's gate view (`RunOutcome` JSON), written by the runner
/// once the whole chain is done — the data a `--wait` caller reads back after
/// the process is gone (REQ-AR-03/07). `--wait` never re-runs a role.
pub const OUTCOME_FILE: &str = "outcome.json";

/// One launch request as the API hands it to a runner.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub home: PathBuf,
    pub task_id: String,
    /// Empty = the agent of the active model bundle (same rule as a manual run).
    pub agent: String,
    pub workspace: Option<PathBuf>,
    pub force_reaudit: bool,
    pub actor: String,
    /// The run number the caller minted; every execution of this chain carries
    /// it (REQ-AR-04/07).
    pub run_id: String,
}

/// What the caller learns. `Started` carries the id of the FIRST execution —
/// the run handle the operator can quote; `AlreadyRunning` carries the id of
/// the execution that already holds the task (REQ-AR-06: the refusal names it
/// and does not touch the first run); `Refused` is «нельзя» with a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchOutcome {
    Started {
        run_id: String,
        execution_id: String,
    },
    AlreadyRunning {
        run_id: String,
        execution_id: String,
    },
    Refused {
        reason: String,
    },
}

/// Why a launch could not even be handed to a runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// No platform runner binary is resolvable — nothing was started.
    NoRunner(String),
    /// `spawn` itself failed (permissions, missing exec bit).
    Spawn(String),
    /// The runner started but never reported within [`ACK_TIMEOUT`]; its log
    /// path is named so the operator can read the real reason.
    Timeout(String),
    /// The runner reported a refusal it could not classify itself.
    Runner(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::NoRunner(m) => write!(f, "{m}"),
            LaunchError::Spawn(m) => write!(f, "cannot start the platform runner: {m}"),
            LaunchError::Timeout(m) => write!(f, "{m}"),
            LaunchError::Runner(m) => write!(f, "platform runner: {m}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// The platform runner binary. Resolution order: `DEVAI_RUNNER` (an explicit
/// path — the UI/diagnostic door), the CURRENT executable when it is the
/// `devai` CLI itself (the normal `devai task run` case), then `devai` on
/// `PATH`. Anything else is NO runner: a launch must never silently run the
/// role in the caller after promising it the role is detached.
pub fn runner_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DEVAI_RUNNER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
        return None;
    }
    if let Ok(exe) = std::env::current_exe() {
        let stem = exe
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        // The CLI binary is `devai` (also under target/<profile>/); a UI or a
        // test harness is NOT a runner and must not be executed as one.
        if stem == "devai" || stem == "devai-cli" {
            return Some(exe);
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join("devai"))
            .find(|p| p.is_file())
    })
}

/// The evidence directory of one run, under the home (never a checkout).
pub fn run_evidence_dir(home: &Path, task_id: &str, run_id: &str) -> PathBuf {
    home.join("evidence").join(task_id).join(run_id)
}

/// The rootless way out of the caller's cgroup (REQ-AR-01): `systemd-run
/// --user --scope` registers the command as a transient USER unit, so the
/// caller's unit can be stopped without taking the role down. Probed once per
/// process; `None` means no user manager answers and the launch keeps the
/// session-only guarantee (`setsid`) with a loud note in the supervisor log.
pub fn systemd_user_scope() -> Option<PathBuf> {
    static PROBED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PROBED
        .get_or_init(|| {
            let exe = std::env::var_os("PATH")
                .and_then(|paths| {
                    std::env::split_paths(&paths)
                        .map(|d| d.join("systemd-run"))
                        .find(|p| p.is_file())
                })
                .or_else(|| {
                    ["/usr/bin/systemd-run", "/bin/systemd-run"]
                        .iter()
                        .map(PathBuf::from)
                        .find(|p| p.is_file())
                })?;
            let ok = std::process::Command::new(&exe)
                .args(["--user", "--scope", "--quiet", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            ok.then_some(exe)
        })
        .clone()
}

/// The ack path of one launch — under the home's tmp, unique per run id.
pub fn ack_path(home: &Path, run_id: &str) -> PathBuf {
    home.join("tmp").join(format!("launch-{run_id}.ack"))
}

/// Start ONE detached platform runner and wait (bounded) for its handshake.
///
/// The child is placed in its OWN session via `setsid(2)` — the mechanism the
/// spec deliberately leaves to the implementation (unknowns). `setsid` needs
/// no privileges, so the role never requires root (REQ-AR-01/17); where the
/// call fails the launch is refused loudly instead of falling back to a
/// same-session child.
pub fn launch(req: &LaunchRequest) -> Result<LaunchOutcome, LaunchError> {
    let exe = runner_exe().ok_or_else(|| {
        LaunchError::NoRunner(
            "no platform runner binary — set DEVAI_RUNNER to the `devai` executable \
             (the role is never run inside the caller)"
                .to_string(),
        )
    })?;
    let ack = ack_path(&req.home, &req.run_id);
    if let Some(parent) = ack.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| LaunchError::Spawn(format!("cannot create {}: {e}", parent.display())))?;
    }
    // A stale ack from a previous, identical run id would be read as this
    // launch's answer — clear it first.
    let _ = std::fs::remove_file(&ack);
    let ev = run_evidence_dir(&req.home, &req.task_id, &req.run_id);
    std::fs::create_dir_all(&ev)
        .map_err(|e| LaunchError::Spawn(format!("cannot create {}: {e}", ev.display())))?;
    let log_path = ev.join(SUPERVISOR_LOG);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| LaunchError::Spawn(format!("cannot open {}: {e}", log_path.display())))?;
    let log_err = log
        .try_clone()
        .map_err(|e| LaunchError::Spawn(format!("cannot clone log handle: {e}")))?;

    // REQ-AR-01: the runner must live OUTSIDE the caller's resource-control
    // area, not merely in another session. A session (`setsid`) survives the
    // caller's terminal, but NOT a systemd unit that is stopped: the default
    // `KillMode=control-group` kills every process in the unit's cgroup,
    // which is exactly the 12/23/39/59-minute incident. The rootless escape
    // is a transient USER scope (`systemd-run --user --scope`): it registers
    // the command as its own unit under the user manager, so stopping the
    // calling unit leaves it alone. No root, no daemon of ours (ASM-5).
    // Where no user manager answers (containers, CI), the launch falls back
    // to `setsid` — the session guarantee — and says so in the supervisor log.
    let scope = systemd_user_scope();
    let mut cmd = match &scope {
        Some(systemd_run) => {
            let mut c = Command::new(systemd_run);
            c.args(["--user", "--scope", "--quiet", "--collect", "--"]);
            c.arg(&exe);
            c
        }
        None => Command::new(&exe),
    };
    cmd.arg("--home")
        .arg(&req.home)
        .arg("--actor")
        .arg(&req.actor)
        .arg(RUNNER_SUBCOMMAND)
        .arg("--task")
        .arg(&req.task_id)
        .arg("--run")
        .arg(&req.run_id)
        .arg("--ack")
        .arg(&ack);
    if !req.agent.is_empty() {
        cmd.arg("--agent").arg(&req.agent);
    }
    if let Some(ws) = &req.workspace {
        cmd.arg("--workspace").arg(ws);
    }
    if req.force_reaudit {
        cmd.arg("--force-reaudit");
    }
    // The runner is the PLATFORM, not a role: it must not be classified as a
    // role session by inheriting the caller's run marker.
    cmd.env_remove("DEVAI_EVIDENCE_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    if scope.is_none() {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| {
                use std::io::Write as _;
                writeln!(
                    f,
                    "launch: no user systemd scope available — the runner gets its own SESSION \
                     (setsid), not its own cgroup; a caller unit that kills its cgroup can still \
                     stop this role"
                )
            });
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and touches no shared state;
            // the closure runs in the forked child before exec. A failure is
            // reported through the child's own failure to write the ack (the
            // caller then refuses the launch — never a same-session fallback).
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| LaunchError::Spawn(format!("{}: {e}", exe.display())))?;

    let deadline = Instant::now() + ACK_TIMEOUT;
    loop {
        if let Some(text) = read_ack(&ack) {
            let _ = std::fs::remove_file(&ack);
            return parse_ack(&req.run_id, &text);
        }
        if let Ok(Some(status)) = child.try_wait() {
            // The runner exited without a handshake: read the tail it wrote —
            // that IS the reason, and it is already in the evidence tree.
            let _ = std::fs::remove_file(&ack);
            let tail = read_tail(&log_path, 20);
            return Err(LaunchError::Runner(if tail.trim().is_empty() {
                format!("runner exited with {status} without reporting an execution")
            } else {
                format!("runner exited with {status}: {tail}")
            }));
        }
        if Instant::now() >= deadline {
            let _ = std::fs::remove_file(&ack);
            return Err(LaunchError::Timeout(format!(
                "no execution reported within {}s — read {}",
                ACK_TIMEOUT.as_secs(),
                log_path.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Write the runner's handshake ATOMICALLY (tmp + rename): the caller either
/// sees a complete answer or none, never half a line.
pub fn write_ack(path: &Path, line: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("ack.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, format!("{line}\n"))?;
    std::fs::rename(&tmp, path)
}

fn read_ack(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .read_to_string(&mut text)
        .ok()?;
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() { None } else { Some(line) }
}

/// Parse one handshake line. Format (fields space-separated, reason LAST so it
/// may contain spaces):
/// `started <run> <exec>` · `busy <run> <exec>` · `refused <text…>`
fn parse_ack(run_id: &str, line: &str) -> Result<LaunchOutcome, LaunchError> {
    if let Some(rest) = line.strip_prefix("started ") {
        let mut it = rest.splitn(2, ' ');
        if let (Some(run), Some(exec)) = (it.next(), it.next()) {
            return Ok(LaunchOutcome::Started {
                run_id: run.to_string(),
                execution_id: exec.to_string(),
            });
        }
    }
    if let Some(rest) = line.strip_prefix("busy ") {
        let mut it = rest.splitn(2, ' ');
        if let (Some(run), Some(exec)) = (it.next(), it.next()) {
            return Ok(LaunchOutcome::AlreadyRunning {
                run_id: run.to_string(),
                execution_id: exec.to_string(),
            });
        }
    }
    if let Some(reason) = line.strip_prefix("refused ") {
        return Ok(LaunchOutcome::Refused {
            reason: reason.to_string(),
        });
    }
    Err(LaunchError::Runner(format!(
        "unparseable handshake for run {run_id}: {line}"
    )))
}

/// Last `lines` lines of a text file — the output TAIL the platform keeps as
/// evidence (the ratified answer: up to 4096 lines per stream).
pub fn read_tail(path: &Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// The ratified tail bound (OQ-1 answer: «4096 строк»).
pub const TAIL_LINES: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_lines_are_parsed_into_typed_outcomes() {
        assert_eq!(
            parse_ack("r", "started r e1").unwrap(),
            LaunchOutcome::Started {
                run_id: "r".into(),
                execution_id: "e1".into()
            }
        );
        assert_eq!(
            parse_ack("r", "busy r e9").unwrap(),
            LaunchOutcome::AlreadyRunning {
                run_id: "r".into(),
                execution_id: "e9".into()
            }
        );
        assert_eq!(
            parse_ack("r", "refused task is in APPROVAL, not a role stage").unwrap(),
            LaunchOutcome::Refused {
                reason: "task is in APPROVAL, not a role stage".into()
            }
        );
        assert!(parse_ack("r", "garbage").is_err());
    }

    #[test]
    fn an_ack_lands_atomically_and_reads_back_whole() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("x.ack");
        write_ack(&p, "started run exec").unwrap();
        assert_eq!(read_ack(&p).as_deref(), Some("started run exec"));
        assert!(
            !p.with_extension("ack.tmp").exists(),
            "temp file is renamed away"
        );
    }

    #[test]
    fn the_tail_keeps_the_last_lines_only() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("log");
        std::fs::write(&p, "a\nb\nc\nd\n").unwrap();
        assert_eq!(read_tail(&p, 2), "c\nd");
        assert_eq!(read_tail(&d.path().join("absent"), 2), "");
    }
}
