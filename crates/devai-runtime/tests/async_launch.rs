//! The platform OWNS the role process: detached spawn + proof-based reclaim
//! (REQ-AR-01/03/05).
//!
//! These tests exercise the two halves that make «работа идёт» a fact rather
//! than a guess:
//! * the runner really lands in its OWN session (so the caller's death cannot
//!   take it down) and its handshake is parsed into the typed handle;
//! * a RUNNING row whose owner is provably gone is closed by the platform with
//!   the right class and the slot is freed — while an UNPROVABLE owner (other
//!   host) is never guessed dead.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use devai_core::types::*;
use devai_runtime::launch::{LaunchOutcome, LaunchRequest, ack_path, launch};
use devai_runtime::reclaim;
use devai_store::Store;

/// `DEVAI_RUNNER` is process-global: one test at a time holds the lock, and
/// the value it FOUND is restored on drop — a panicking assert must not leave
/// a stub runner behind for the rest of the binary (rework R8).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The scoped, panic-safe `DEVAI_RUNNER` write: see [`ENV_LOCK`].
struct RunnerEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl RunnerEnv {
    fn set(path: &Path) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("DEVAI_RUNNER");
        // SAFETY: `ENV_LOCK` serialises this file's only reader, and the
        // previous value is put back on drop.
        unsafe { std::env::set_var("DEVAI_RUNNER", path) };
        Self {
            _lock: guard,
            previous,
        }
    }
}

impl Drop for RunnerEnv {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(v) => unsafe { std::env::set_var("DEVAI_RUNNER", v) },
            None => unsafe { std::env::remove_var("DEVAI_RUNNER") },
        }
    }
}

fn session_of(pid: i64) -> i64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let tail = stat.rfind(')').unwrap();
    let fields: Vec<&str> = stat[tail + 1..].split_whitespace().collect();
    // fields[0] = field 3 (state); session id is field 6 => index 3.
    fields[3].parse().unwrap()
}

fn cgroup_of(pid: i64) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[test]
fn the_runner_gets_its_own_session_and_hands_back_a_handle() {
    let home = tempfile::TempDir::new().unwrap();
    let script = home.path().join("fake-runner.sh");
    let sid_file = home.path().join("sid");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             sid=$(awk '{{print $6}}' /proc/$$/stat)\n\
             echo \"$$ $sid\" > {sid}\n\
             ack=\"\"; run=\"\"; prev=\"\"\n\
             for a in \"$@\"; do\n\
               [ \"$prev\" = \"--ack\" ] && ack=\"$a\"\n\
               [ \"$prev\" = \"--run\" ] && run=\"$a\"\n\
               prev=\"$a\"\n\
             done\n\
             printf 'started %s 01890000-0000-7000-8000-000000000001\\n' \"$run\" > \"$ack\"\n",
            sid = sid_file.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let _env = RunnerEnv::set(&script);
    let run_id = new_id().to_string();
    let req = LaunchRequest {
        home: home.path().to_path_buf(),
        task_id: "task".into(),
        agent: String::new(),
        workspace: None,
        force_reaudit: false,
        actor: "tester".into(),
        run_id: run_id.clone(),
    };
    let out = launch(&req).expect("handshake");

    match out {
        LaunchOutcome::Started {
            caller_run_id: r,
            execution_id,
        } => {
            assert_eq!(r, run_id);
            assert_eq!(execution_id, "01890000-0000-7000-8000-000000000001");
        }
        other => panic!("expected a started handle, got {other:?}"),
    }
    // The ack is consumed by the caller: a stale one must never answer a
    // later launch (REQ-AR-08).
    assert!(!ack_path(home.path(), &run_id).exists());
    // REQ-AR-01 AC1: the runner is OUTSIDE the caller's control group — the
    // caller unit's cgroup kill cannot reach it. Where a user systemd scope is
    // available that is the guarantee; otherwise the fallback is a separate
    // SESSION (`setsid`), which survives a terminal but not a cgroup kill.
    let probe = std::fs::read_to_string(&sid_file).unwrap();
    let mut it = probe.split_whitespace();
    let child_pid: i64 = it.next().unwrap().parse().unwrap();
    let child_sid: i64 = it.next().unwrap().parse().unwrap();
    if devai_runtime::launch::systemd_user_scope().is_some() {
        assert_ne!(
            cgroup_of(child_pid),
            cgroup_of(std::process::id() as i64),
            "the runner must live in its own transient scope (cgroup)"
        );
    } else {
        assert_ne!(
            child_sid,
            session_of(std::process::id() as i64),
            "the runner must live in its own session (setsid)"
        );
    }
}

fn exec_row(task_id: Id, owner: (Option<i64>, Option<String>, Option<i64>)) -> RoleExecution {
    RoleExecution {
        id: new_id(),
        task_id,
        role_slug: "requirements-analyst".into(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: ExecutionStatus::Running,
        inputs: Vec::new(),
        summary: None,
        error: None,
        started_at: Utc::now(),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: owner.0,
        owner_host: owner.1,
        owner_process_start: owner.2,
        bundle: None,
        run_id: None,
    }
}

struct Home {
    dir: tempfile::TempDir,
    store: Store,
}

fn home() -> Home {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    Home { dir, store }
}

fn task_in(store: &Store, title: &str) -> Id {
    let t = Task::new(title, "", None, Utc::now());
    store.create_task(&t).unwrap();
    t.id
}

#[test]
fn a_provably_dead_owner_is_closed_with_its_class_and_the_slot_frees() {
    let h = home();
    let task = task_in(&h.store, "dead owner");
    let host = devai_runtime::current_host();
    // PID i64::MAX does not exist on this host: the ONE provable death.
    let dead = exec_row(
        task,
        (Some(i64::MAX), host.clone(), Some(Utc::now().timestamp())),
    );
    h.store.start_execution(&dead).unwrap();

    let closed = reclaim::reclaim(&h.store, Some(task)).unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].class, failure_class::PROCESS_VANISHED);
    let row = h.store.get_execution(&dead.id.to_string()).unwrap();
    assert_eq!(row.status, ExecutionStatus::Failed);
    assert_eq!(
        row.failure_class.as_deref(),
        Some(failure_class::PROCESS_VANISHED)
    );
    // The proof is in the append-only log, not only on the row.
    assert!(
        h.store
            .events(task, None)
            .unwrap()
            .iter()
            .any(|e| e.kind == event_kind::EXECUTION_LOST),
        "the closure must be an execution.lost event"
    );
    // ...and the single-writer slot is free: a new run can open its row.
    let fresh = exec_row(task, (Some(std::process::id() as i64), host, None));
    h.store.start_execution(&fresh).expect("slot is free");
}

#[test]
fn a_row_started_before_this_boot_is_the_host_restarted_class() {
    let h = home();
    let task = task_in(&h.store, "rebooted");
    let mut row = exec_row(
        task,
        (Some(i64::MAX), devai_runtime::current_host(), Some(1)),
    );
    // Epoch 0 predates any boot: nothing of this row can still be running.
    row.started_at = chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap();
    h.store.start_execution(&row).unwrap();
    let closed = reclaim::reclaim(&h.store, Some(task)).unwrap();
    assert_eq!(closed[0].class, failure_class::HOST_RESTARTED);
}

#[test]
fn an_unprovable_owner_is_left_running_and_a_pre_owner_row_is_closed_by_the_ratified_rule() {
    let h = home();
    // Another host: not observable here — never guessed dead (REQ-AR-05 AC2).
    let foreign_task = task_in(&h.store, "foreign host");
    let foreign = exec_row(
        foreign_task,
        (Some(1), Some("some-other-host".into()), None),
    );
    h.store.start_execution(&foreign).unwrap();
    // Pre-ownership row: RUNNING with no owner columns at all. The ratified
    // answer closes exactly these as lost_before_ownership.
    let legacy_task = task_in(&h.store, "legacy row");
    let legacy = exec_row(legacy_task, (None, None, None));
    h.store.start_execution(&legacy).unwrap();

    let closed = reclaim::reclaim(&h.store, None).unwrap();
    let classes: Vec<&str> = closed.iter().map(|c| c.class).collect();
    assert_eq!(classes, vec![failure_class::LOST_BEFORE_OWNERSHIP]);
    assert_eq!(
        h.store
            .get_execution(&legacy.id.to_string())
            .unwrap()
            .status,
        ExecutionStatus::Failed
    );
    assert_eq!(
        h.store
            .get_execution(&foreign.id.to_string())
            .unwrap()
            .status,
        ExecutionStatus::Running,
        "an unprovable owner keeps its slot and stays nameable for a human"
    );
}

#[test]
fn the_tail_helper_keeps_the_last_lines() {
    let d = tempfile::TempDir::new().unwrap();
    let p = Path::new(d.path()).join("log");
    std::fs::write(&p, "1\n2\n3\n").unwrap();
    assert_eq!(
        devai_runtime::tail::read_tail(&p, 2),
        Ok("2\n3".to_string())
    );
    // An absent file is a TYPED miss, never an empty tail (QA R3).
    assert!(matches!(
        devai_runtime::tail::read_tail(&d.path().join("absent"), 2),
        Err(devai_runtime::tail::TailError::NotFound(_))
    ));
}
