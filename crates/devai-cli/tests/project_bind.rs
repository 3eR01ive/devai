//! C5 D1 (REQ-BIND-01/02/03): the project-binding surface through the REAL
//! binary. Pins the ratified OQ-BIND-1 default (working directory matched
//! against the projects' `repos` — explicit `--project` keeps its exact old
//! path and always wins; ambiguity binds nothing), the machine-readable
//! `project_id` on the creation output, the `task set-project` write with
//! its append-only event + idempotent replay, and `task backfill-project`
//! under the ratified non-terminal-only rule. All cross-step references
//! travel as FULL ids (REQ-E2E-01).

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run the binary with an EXPLICIT working directory — the cwd is the very
/// input the ratified default mechanism resolves against, so every call
/// here names it deliberately (`home` stays the store dir, not the cwd).
fn devai(home: &Path, cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("bind-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(
        out.status.success(),
        "devai {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn devai_json(home: &Path, cwd: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json"];
    full.extend(args.iter().copied());
    let s = devai(home, cwd, &full);
    serde_json::from_str(s.trim()).unwrap_or_else(|e| {
        panic!("devai {args:?} --json must print exactly one JSON object ({e}): {s}")
    })
}

fn devai_fail(home: &Path, cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("bind-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(
        out.status.code() != Some(0),
        "devai {args:?} unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// (home, the registered repo dir, a dir OUTSIDE every repo) — every
/// scenario names its cwd explicitly, the cwd IS the mechanism input.
fn fixture() -> (TempDir, TempDir, TempDir, TempDir) {
    let home = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    devai(home.path(), repo.path(), &["init"]);
    (home, repo, elsewhere, TempDir::new().unwrap())
}

fn project_id(home: &Path, name: &str) -> String {
    devai_json(home, Path::new("."), &["project", "get", name])["id"]
        .as_str()
        .expect("project id")
        .to_string()
}

/// REQ-BIND-01 AC1 + AC3: a task created from inside the registered repo
/// is BORN bound to that project (the ratified repo=home default), and the
/// machine-readable creation output carries `project_id` with the FULL id.
/// A subdir of the repo binds the same project; a foreign cwd keeps the
/// task projectless (yesterday's behavior where the rule does not speak).
#[test]
fn task_new_binds_the_project_of_the_working_directory() {
    let (home, repo, elsewhere, _x) = fixture();
    let h = home.path();
    let repo_path = repo.path().canonicalize().unwrap();
    devai(
        h,
        &elsewhere.path().to_path_buf(),
        &[
            "project",
            "add",
            "devai",
            "--repo",
            &repo_path.display().to_string(),
        ],
    );
    let pid = project_id(h, "devai");

    // From the repo itself: bound.
    let created = devai_json(h, &repo_path, &["task", "new", "auto-bound"]);
    assert_eq!(created["project_id"].as_str(), Some(pid.as_str()));
    // The FULL id round-trips through task get (no short-id references).
    let full = created["id"].as_str().unwrap().to_string();
    assert_eq!(
        devai_json(h, &elsewhere.path().to_path_buf(), &["task", "get", &full])["project_id"]
            .as_str(),
        Some(pid.as_str())
    );

    // From a subdirectory: still inside the repo — bound.
    let subdir = repo_path.join("crates").join("devai-core");
    std::fs::create_dir_all(&subdir).unwrap();
    let created = devai_json(h, &subdir, &["task", "new", "bound-from-subdir"]);
    assert_eq!(created["project_id"].as_str(), Some(pid.as_str()));

    // From an unrelated directory: nothing matches — projectless, exactly
    // the pre-D1 shape (the default never fabricates a project).
    let created = devai_json(h, elsewhere.path(), &["task", "new", "projectless"]);
    assert!(created["project_id"].is_null(), "{created}");
}

/// REQ-BIND-01 AC2: explicit `--project` outranks the cwd default (the
/// truth source when both speak), and its not-found refusal stays VERBATIM
/// where it always was — the ratified default must not move it.
/// Ratified OQ-BIND-1: AMBIGUITY (two projects over one repo) binds
/// NOTHING automatically; the explicit flag is then the only way to bind.
#[test]
fn explicit_project_wins_and_ambiguity_binds_nothing() {
    let (home, repo, elsewhere, _x) = fixture();
    let h = home.path();
    let repo_path = repo.path().canonicalize().unwrap();
    let r = repo_path.display().to_string();
    devai(
        h,
        elsewhere.path(),
        &["project", "add", "devai", "--repo", &r],
    );
    devai(h, elsewhere.path(), &["project", "add", "alpha"]);
    let devai_id = project_id(h, "devai");
    let alpha_id = project_id(h, "alpha");

    // Explicit flag from inside the repo: the NAMED project wins over the
    // cwd default.
    let created = devai_json(
        h,
        &repo_path,
        &["task", "new", "explicit-wins", "--project", "alpha"],
    );
    assert_eq!(created["project_id"].as_str(), Some(alpha_id.as_str()));

    // The refusal text and exit path are byte-stable (dogfood ritual
    // unchanged): unknown name still fails at resolve, not in the store.
    let err = devai_fail(
        h,
        &repo_path,
        &["task", "new", "boom", "--project", "no-such-project"],
    );
    assert!(err.contains("project `no-such-project` not found"), "{err}");

    // Two projects claim the same repo: the default cannot pick — it binds
    // nothing, and the operator's explicit flag resolves the ambiguity.
    devai(
        h,
        elsewhere.path(),
        &["project", "add", "beta", "--repo", &r],
    );
    let created = devai_json(h, &repo_path, &["task", "new", "ambiguous"]);
    assert!(created["project_id"].is_null(), "{created}");
    let created = devai_json(
        h,
        &repo_path,
        &["task", "new", "resolved-explicitly", "--project", "devai"],
    );
    assert_eq!(created["project_id"].as_str(), Some(devai_id.as_str()));
}

/// REQ-BIND-02 (AC1..AC4) through the binary: `task set-project` writes
/// the binding, bumps the lock, appends `task.project.set` to the
/// append-only history (the row is never rewritten — replay adds NOTHING),
/// shows the project in the human `task get`, and both ref families refuse
/// typed. `--json` prints the machine outcome.
#[test]
fn set_project_write_lands_event_and_replays_clean() {
    let (home, repo, elsewhere, _x) = fixture();
    let h = home.path();
    let repo_path = repo.path().canonicalize().unwrap();
    devai(
        h,
        elsewhere.path(),
        &[
            "project",
            "add",
            "devai",
            "--repo",
            &repo_path.display().to_string(),
        ],
    );
    let pid = project_id(h, "devai");

    // A task born projectless (foreign cwd), then bound explicitly.
    let created = devai_json(h, elsewhere.path(), &["task", "new", "late-binding"]);
    let full = created["id"].as_str().unwrap().to_string();
    assert!(created["project_id"].is_null());

    let o = devai_json(
        h,
        elsewhere.path(),
        &["task", "set-project", &full, "devai"],
    );
    assert_eq!(o["changed"].as_bool(), Some(true));
    assert_eq!(o["task"]["project_id"].as_str(), Some(pid.as_str()));
    assert_eq!(o["task"]["lock_version"].as_u64(), Some(2));

    // The human `task get` shows the project (REQ-BIND-02 AC4).
    let human = devai(h, elsewhere.path(), &["task", "get", &full]);
    assert!(human.contains("project:"), "{human}");

    // The event is append-only history, payload speaking the vocabulary.
    let evs = devai_json(h, elsewhere.path(), &["task", "events", &full]);
    let set = evs
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "task.project.set")
        .expect("the binding left its event");
    assert_eq!(set["payload"]["project_id"].as_str(), Some(pid.as_str()));
    assert!(set["payload"]["previous_project_id"].is_null());
    let n_before = evs.as_array().unwrap().len();

    // Re-apply the SAME project: documented NO-OP — exit 0, changed=false,
    // the lock does NOT move, no new event row appears.
    let o = devai_json(
        h,
        elsewhere.path(),
        &["task", "set-project", &full, "devai"],
    );
    assert_eq!(o["changed"].as_bool(), Some(false));
    assert_eq!(o["task"]["lock_version"].as_u64(), Some(2));
    let evs = devai_json(h, elsewhere.path(), &["task", "events", &full]);
    assert_eq!(
        evs.as_array().unwrap().len(),
        n_before,
        "replay wrote nothing"
    );

    // Typed refusals, nothing written on either path.
    let err = devai_fail(
        h,
        elsewhere.path(),
        &["task", "set-project", "no-such-task", "devai"],
    );
    assert!(err.contains("task `no-such-task` not found"), "{err}");
    let err = devai_fail(
        h,
        elsewhere.path(),
        &["task", "set-project", &full, "no-such-project"],
    );
    assert!(err.contains("project `no-such-project` not found"), "{err}");
    assert_eq!(
        devai_json(h, elsewhere.path(), &["task", "get", &full])["lock_version"].as_u64(),
        Some(2)
    );
}

/// REQ-BIND-03 (ratified OQ-BIND-2: only non-terminal tasks):
/// `task backfill-project` binds every unbound NON-TERMINAL task through
/// the same operation (own event each), leaves terminal history at its old
/// address, never re-binds another project's task, and changes nothing on
/// re-application (idempotent).
#[test]
fn backfill_project_binds_open_history_only_and_is_idempotent() {
    let (home, repo, elsewhere, _x) = fixture();
    let h = home.path();
    let repo_path = repo.path().canonicalize().unwrap();
    devai(
        h,
        elsewhere.path(),
        &[
            "project",
            "add",
            "devai",
            "--repo",
            &repo_path.display().to_string(),
        ],
    );
    devai(h, elsewhere.path(), &["project", "add", "alpha"]);
    let pid = project_id(h, "devai");
    let alpha_id = project_id(h, "alpha");

    // The store shape: unbound DRAFT, unbound CANCELLED (terminal), and a
    // task owned by another project. All created projectless (foreign cwd).
    let free = devai_json(h, elsewhere.path(), &["task", "new", "queue-open"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let closed = devai_json(h, elsewhere.path(), &["task", "new", "closed-history"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    devai(
        h,
        elsewhere.path(),
        &["task", "cancel", &closed, "--reason", "done elsewhere"],
    );
    let owned = devai_json(h, elsewhere.path(), &["task", "new", "alpha-owned"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    devai(
        h,
        elsewhere.path(),
        &["task", "set-project", &owned, "alpha"],
    );

    let o = devai_json(h, elsewhere.path(), &["task", "backfill-project", "devai"]);
    assert_eq!(o["bound"], serde_json::json!([free]));
    assert_eq!(o["skipped_terminal"], serde_json::json!([closed]));
    assert_eq!(o["skipped_other_project"], serde_json::json!([owned]));

    let listed = devai_json(h, elsewhere.path(), &["task", "list"]);
    let by_id = |id: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_id(&free)["project_id"].as_str(), Some(pid.as_str()));
    assert!(
        by_id(&closed)["project_id"].is_null(),
        "terminal history keeps the address it lived at"
    );
    assert_eq!(
        by_id(&owned)["project_id"].as_str(),
        Some(alpha_id.as_str())
    );
    assert_eq!(by_id(&free)["lock_version"].as_u64(), Some(2));

    // Idempotent re-application: no candidates, no movement anywhere.
    let o = devai_json(h, elsewhere.path(), &["task", "backfill-project", "devai"]);
    assert_eq!(o["bound"], serde_json::json!([]));
    assert_eq!(by_id(&free)["lock_version"].as_u64(), Some(2));

    // Unknown project: typed not-found refusal, exit nonzero, no writes.
    let err = devai_fail(
        h,
        elsewhere.path(),
        &["task", "backfill-project", "no-such"],
    );
    assert!(err.contains("project `no-such` not found"), "{err}");
}
