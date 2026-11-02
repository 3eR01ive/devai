//! REQ-WT-07: the MERGE GATE through the real `devai` binary. Merging the
//! platform-created `rework/<task-id>` branch into the baseline is a human
//! act and the ONLY merge path: it records an attributable decision + event,
//! moves no task state (workflow.rs: Accepted has no merge operation), and
//! fails loudly when the task has no branch — there is no fallback and no
//! platform-side merge for agents to discover.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("merge-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
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

fn devai_json(home: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json"];
    full.extend(args.iter().copied());
    let s = devai(home, &full);
    serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("devai {args:?} --json must print JSON ({e}): {s}"))
}

fn devai_fail(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("merge-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(!out.status.success(), "devai {args:?} must fail, got stdout: {}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_ok(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git is available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn merge_gate_is_the_only_path_into_the_baseline() {
    let home = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    git_ok(ws.path(), &["init", "-q"]);
    std::fs::write(ws.path().join("tracked.txt"), b"alpha\n").unwrap();
    git_ok(ws.path(), &["add", "tracked.txt"]);
    git_ok(
        ws.path(),
        &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"],
    );
    let head = git(ws.path(), &["rev-parse", "HEAD"]);

    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "Merge gate", "--description", "REQ-WT-07 end to end"]);
    let id = created["id"].as_str().unwrap().to_string();
    let ws_str = ws.path().to_str().unwrap();
    devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    devai(home.path(), &["task", "approve", &id, "--rationale", "merge-gate test"]);
    devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);

    let branch = format!("rework/{id}");
    let wt = home.path().join("worktrees").join(&id);
    assert!(wt.is_dir(), "the writing stage ran inside the platform worktree");
    assert_eq!(git(ws.path(), &["rev-parse", &branch]), head, "the platform branch sits at the bound commit, baseline unmoved");

    // Simulate a real captured writing run: committed work on the task
    // branch. In production `worktree::commit_platform_work` creates this
    // commit; the mock agent changes nothing, so the test performs the
    // repository fact by hand INSIDE the platform's own worktree — it is
    // exactly what the gate consumes.
    std::fs::write(wt.join("feature.txt"), b"delivered\n").unwrap();
    git_ok(&wt, &["add", "feature.txt"]);
    git_ok(
        &wt,
        &["-c", "user.email=devai@platform.local", "-c", "user.name=devai-platform", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "devai: implementation: feature"],
    );
    let tip = git(&wt, &["rev-parse", "HEAD"]);

    // The human merges — the single path (REQ-WT-07 AC1).
    let out = devai_json(home.path(), &["task", "merge", &id, "--workspace", ws_str, "--rationale", "accepted work"]);
    assert_eq!(out["head_before"], head);
    assert_eq!(out["head_after"], tip);
    assert_eq!(git(ws.path(), &["rev-parse", "HEAD"]), tip, "the baseline advanced to the task work");
    assert!(ws.path().join("feature.txt").exists(), "the delivered file lands in the baseline only now, by human order");

    // The merge moves no task state: it is a repository act, not a workflow
    // operation (workflow.rs documents exactly that).
    let task = devai_json(home.path(), &["task", "status", &id]);
    let state = task["task"]["state"].as_str().or_else(|| task["state"].as_str()).expect("status reports the state");
    assert_eq!(state, "QA", "merging changes history, not the workflow");

    // Attributable history: one decision + one event, actor = the human who
    // ran the command (vocabulary owned by devai-core, not inline strings).
    let store = devai_store::Store::open(home.path()).unwrap();
    let t = store.resolve_task(&id).unwrap();
    let decs = store.list_decisions(t.id).unwrap();
    let d = decs.iter().find(|d| d.kind == devai_core::types::decision_kind::REWORK_MERGE).expect("merge decision recorded");
    assert_eq!(d.actor, "merge-human");
    assert!(d.refs.contains(&branch) && d.refs.contains(&tip), "the decision cites the exact refs: {:?}", d.refs);
    let events = store.all_events_for_task(t.id).unwrap();
    let merged = events.iter().find(|e| e.kind == devai_core::types::event_kind::REWORK_MERGED).expect("merge event recorded");
    assert_eq!(merged.payload["head_before"], head);
    assert_eq!(merged.payload["head_after"], tip);

    // OQ-1: the platform does not automate cleanup — branch and worktree
    // persist after the merge for the human's lifecycle decision.
    git_ok(ws.path(), &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")]);
    assert!(wt.is_dir(), "OQ-1: no automated worktree removal");

    // A task that never reached a writing stage has nothing to merge: the
    // command fails explicitly and names the branch it looked for. No
    // silent fallback, no other merge path exists (REQ-WT-07 AC2).
    let t2 = devai_json(home.path(), &["task", "new", "No branch", "--description", "never implemented"]);
    let id2 = t2["id"].as_str().unwrap().to_string();
    let err = devai_fail(home.path(), &["task", "merge", &id2, "--workspace", ws_str]);
    assert!(err.contains(&format!("rework/{id2}")), "the failure names the missing branch: {err}");
    assert_eq!(git(ws.path(), &["rev-parse", "HEAD"]), tip, "the refused merge moved nothing");
}
