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

/// How often [`wait_for_handshake`] re-reads the ack file while it waits.
/// A bounded poll of a FILE, never a read on a pipe: no pipe survives the
/// caller, so no SIGPIPE can turn a launch into a phantom.
pub const ACK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The runner's own transcript, under the run's evidence directory.
pub const SUPERVISOR_LOG: &str = "supervisor.log";

/// The settled run's gate view (`RunOutcome` JSON), written by the runner
/// once the whole chain is done — the data a `--wait` caller reads back after
/// the process is gone (REQ-AR-03/07). `--wait` never re-runs a role.
pub const OUTCOME_FILE: &str = "outcome.json";

/// The runner's OWN record of its exit: the exit code plus the bounded tail of
/// its transcript (REQ-AR-03). Written by `__supervise`, read by `exec get` —
/// the platform's outcome survives the caller and is visible without it.
pub const RUNNER_TAIL_FILE: &str = "role_output_tail.txt";

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
///
/// `caller_run_id` is the number THIS call minted and handed to its runner —
/// never the owner's run. In `AlreadyRunning` the owner is named by
/// `execution_id` alone; the caller's own run number never became a run, so
/// reading it as «the run that is going» would be wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchOutcome {
    Started {
        caller_run_id: String,
        execution_id: String,
    },
    AlreadyRunning {
        caller_run_id: String,
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
    /// REQ-AR-01 AC1: no way to place the runner OUTSIDE the caller's
    /// resource-control area while the caller IS a systemd unit. Refused
    /// typed — a same-session child would be swept with the unit.
    NoScope(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::NoRunner(m) => write!(f, "{m}"),
            LaunchError::Spawn(m) => write!(f, "cannot start the platform runner: {m}"),
            LaunchError::Timeout(m) => write!(f, "{m}"),
            LaunchError::Runner(m) => write!(f, "platform runner: {m}"),
            LaunchError::NoScope(m) => write!(f, "{m}"),
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

/// The tail bound lives in ONE place (`crate::tail`); re-exported here so
/// every historical caller path keeps working against the single rule.
pub use crate::tail::{TAIL_LINES, TailError, read_tail};

/// How many lines of the runner's own transcript the launch error quotes —
/// a diagnostic slice, deliberately far below [`TAIL_LINES`] (the evidence
/// tail keeps the rest).
const DIAGNOSTIC_TAIL_LINES: usize = 20;

/// The files of one launch handshake.
struct Handshake {
    ack: PathBuf,
    log: PathBuf,
}

/// Create the evidence dir, the ack dir and clear a stale ack of the same run
/// id. A failed preparation refuses the launch — nothing was started.
fn prepare_handshake(req: &LaunchRequest) -> Result<Handshake, LaunchError> {
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
    Ok(Handshake {
        ack,
        log: ev.join(SUPERVISOR_LOG),
    })
}

/// WHERE the runner lands (REQ-AR-01 AC1). Two acceptable answers and one
/// refusal, decided from facts:
///
/// * `Some(systemd-run)` — the runner becomes its own transient USER unit, so
///   stopping the calling unit cannot reach it. No root (a user scope).
/// * no user manager + the caller is NOT a unit (`INVOCATION_ID` absent): the
///   caller owns no cgroup anyone would sweep, and `setsid` already makes the
///   runner its own session — that IS the guarantee the AC asks for.
/// * no user manager + the caller IS a systemd unit: a session would still die
///   with the unit's cgroup — exactly the 12/23/39/59-minute incident. This is
///   refused typed, never reported as «started».
fn scope_decision(
    user_scope: Option<PathBuf>,
    caller_is_unit: bool,
) -> Result<Option<PathBuf>, LaunchError> {
    match (user_scope, caller_is_unit) {
        (Some(scope), _) => Ok(Some(scope)),
        (None, false) => Ok(None),
        (None, true) => Err(LaunchError::NoScope(format!(
            "the caller runs inside a systemd unit and no user manager answered, so the role \
             cannot be placed outside the caller's cgroup — a same-session child would be killed \
             with the unit (REQ-AR-01). Start the platform from a user session, or enable the \
             user manager (`systemctl --user`)"
        ))),
    }
}

/// The rootless marker of «my process is a unit's main process»: systemd
/// exports `INVOCATION_ID` (and `JOURNAL_STREAM`) to every service it starts.
fn caller_runs_inside_a_unit() -> bool {
    std::env::var_os("INVOCATION_ID").is_some() || std::env::var_os("JOURNAL_STREAM").is_some()
}

/// The two handles the runner writes into: its own transcript (stdout) and a
/// clone for stderr. Opened HERE so a broken evidence path refuses the launch
/// before a child exists.
fn supervisor_sink(path: &Path) -> Result<(std::fs::File, std::fs::File), LaunchError> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| LaunchError::Spawn(format!("cannot open {}: {e}", path.display())))?;
    let err = log
        .try_clone()
        .map_err(|e| LaunchError::Spawn(format!("cannot clone log handle: {e}")))?;
    Ok((log, err))
}

/// The command head: the runner itself, or the transient user scope wrapping
/// it (see [`scope_decision`]).
fn scope_command(exe: &Path, scope: Option<&Path>) -> Command {
    match scope {
        Some(systemd_run) => {
            let mut c = Command::new(systemd_run);
            c.args(["--user", "--scope", "--quiet", "--collect", "--"]);
            c.arg(exe);
            c
        }
        None => Command::new(exe),
    }
}

/// Build the runner command line. The runner is the PLATFORM, not a role, so
/// it must not inherit the caller's run marker.
fn runner_command(
    exe: &Path,
    req: &LaunchRequest,
    hs: &Handshake,
    scope: Option<&Path>,
) -> Result<Command, LaunchError> {
    let (log, log_err) = supervisor_sink(&hs.log)?;
    let mut cmd = scope_command(exe, scope);
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
        .arg(&hs.ack);
    if !req.agent.is_empty() {
        cmd.arg("--agent").arg(&req.agent);
    }
    if let Some(ws) = &req.workspace {
        cmd.arg("--workspace").arg(ws);
    }
    if req.force_reaudit {
        cmd.arg("--force-reaudit");
    }
    cmd.env_remove("DEVAI_EVIDENCE_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    Ok(cmd)
}

/// Give the child its own SESSION when no user scope is available. `setsid`
/// needs no privileges, so the role never requires root (REQ-AR-01/17); a
/// failure surfaces as the child's inability to write the ack (the caller then
/// refuses the launch — never a same-session fallback).
fn detach_into_own_session(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no shared state;
        // the closure runs in the forked child before exec.
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

/// Bounded wait for the runner's handshake. Three exits: the handshake, the
/// runner dying without one (its own tail IS the reason) and the timeout.
fn wait_for_handshake(
    child: &mut std::process::Child,
    hs: &Handshake,
) -> Result<LaunchOutcome, LaunchError> {
    let deadline = Instant::now() + ACK_TIMEOUT;
    loop {
        if let Some(text) = read_ack(&hs.ack) {
            let _ = std::fs::remove_file(&hs.ack);
            return parse_ack(&text);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let _ = std::fs::remove_file(&hs.ack);
            return Err(runner_died(&hs.log, status));
        }
        if Instant::now() >= deadline {
            let _ = std::fs::remove_file(&hs.ack);
            return Err(LaunchError::Timeout(format!(
                "no execution reported within {}s — read {}",
                ACK_TIMEOUT.as_secs(),
                hs.log.display()
            )));
        }
        std::thread::sleep(ACK_POLL_INTERVAL);
    }
}

/// The runner exited before reporting: its transcript tail is the reason. An
/// unreadable transcript is NAMED as such — never dressed up as «no output».
fn runner_died(log: &Path, status: std::process::ExitStatus) -> LaunchError {
    match read_tail(log, DIAGNOSTIC_TAIL_LINES) {
        Ok(tail) if tail.trim().is_empty() => LaunchError::Runner(format!(
            "runner exited with {status} without reporting an execution"
        )),
        Ok(tail) => LaunchError::Runner(format!("runner exited with {status}: {tail}")),
        Err(e) => LaunchError::Runner(format!(
            "runner exited with {status} without reporting an execution, and its transcript is \
             unreadable ({e})"
        )),
    }
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
    // REQ-AR-01 AC1: a session INSIDE the caller's cgroup is not a guarantee.
    let scope = scope_decision(systemd_user_scope(), caller_runs_inside_a_unit())?;
    let hs = prepare_handshake(req)?;
    let mut cmd = runner_command(&exe, req, &hs, scope.as_deref())?;
    if scope.is_none() {
        note_session_scope(&hs.log);
        detach_into_own_session(&mut cmd);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| LaunchError::Spawn(format!("{}: {e}", exe.display())))?;
    wait_for_handshake(&mut child, &hs)
}

/// Record in the run's own transcript WHICH boundary the runner got. The
/// session branch only happens when the caller owns no cgroup (see
/// [`scope_decision`]), and saying so keeps the evidence honest.
fn note_session_scope(log: &Path) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|mut f| {
            use std::io::Write as _;
            writeln!(
                f,
                "launch: no user systemd manager — the runner gets its own SESSION (setsid); the \
                 caller is not a systemd unit, so no unit cgroup can sweep the role"
            )
        });
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
fn parse_ack(line: &str) -> Result<LaunchOutcome, LaunchError> {
    if let Some(rest) = line.strip_prefix("started ") {
        let mut it = rest.splitn(2, ' ');
        if let (Some(run), Some(exec)) = (it.next(), it.next()) {
            return Ok(LaunchOutcome::Started {
                caller_run_id: run.to_string(),
                execution_id: exec.to_string(),
            });
        }
    }
    if let Some(rest) = line.strip_prefix("busy ") {
        let mut it = rest.splitn(2, ' ');
        if let (Some(run), Some(exec)) = (it.next(), it.next()) {
            return Ok(LaunchOutcome::AlreadyRunning {
                caller_run_id: run.to_string(),
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
        "unparseable launch handshake: {line}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_lines_are_parsed_into_typed_outcomes() {
        assert_eq!(
            parse_ack("started r e1").unwrap(),
            LaunchOutcome::Started {
                caller_run_id: "r".into(),
                execution_id: "e1".into()
            }
        );
        assert_eq!(
            parse_ack("busy r e9").unwrap(),
            LaunchOutcome::AlreadyRunning {
                caller_run_id: "r".into(),
                execution_id: "e9".into()
            }
        );
        assert_eq!(
            parse_ack("refused task is in APPROVAL, not a role stage").unwrap(),
            LaunchOutcome::Refused {
                reason: "task is in APPROVAL, not a role stage".into()
            }
        );
        assert!(parse_ack("garbage").is_err());
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

    /// QA R4 AC3 / R3: a session inside the caller's cgroup is NOT accepted
    /// when the caller is a unit — the launch is refused, never «started».
    #[test]
    fn a_unit_caller_without_a_user_scope_is_refused_typed() {
        let refused = scope_decision(None, true).unwrap_err();
        assert!(matches!(refused, LaunchError::NoScope(_)));
        // No user manager, but the caller owns no unit cgroup: a session is
        // the honest guarantee.
        assert_eq!(scope_decision(None, false), Ok(None));
        // A user scope always wins.
        let scope = PathBuf::from("/usr/bin/systemd-run");
        assert_eq!(scope_decision(Some(scope.clone()), true), Ok(Some(scope)));
    }
}
