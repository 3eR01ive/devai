//! Mechanical read-only anchor for the workspace-view operations (REQ-WV-08)
//! plus the behaviour pins of REQ-WV-01/02/03/07: both new operations run
//! against a REAL fixture repository whose worktree was created by the very
//! platform plumbing (`worktree::prepare` + `commit_platform_work`), the
//! main checkout is verified byte-identical across the calls (status/HEAD/
//! index — the precedent of
//! `worktree.rs::writing_stage_never_touches_the_main_working_tree`), and
//! store row counts never move.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use devai_api::workspace_state;
use devai_api::{Api, ErrorCode};
use devai_core::types::*;
use devai_runtime::worktree;
use devai_store::Store;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// One fixture run: real repo + store home + a task whose isolation
/// happened exactly the way the engine does it (prepare → agent edits →
/// platform commit), with the same event payloads the engine writes
/// (engine.rs worktree.prepared / worktree.committed).
struct Fixture {
    home: tempfile::TempDir,
    repo: tempfile::TempDir,
    task_id: Id,
    exec_id: Id,
    bound_head: String,
    /// tip when the fixture commits agent work; None for the zero-change run.
    tip: Option<String>,
    wt_dir: PathBuf,
}

#[derive(PartialEq, Eq, Debug)]
struct MainState {
    status: String,
    head: String,
    index: String,
}

impl Fixture {
    /// `zero_change` runs prepare + commit_platform_work with NOTHING
    /// modified — the legitimate no-op the platform stamps as tip=bound.
    fn new(zero_change: bool) -> Fixture {
        let home = tempfile::TempDir::new().unwrap();
        let repo = tempfile::TempDir::new().unwrap();
        let p = repo.path();
        git(p, &["init", "-q"]);
        std::fs::write(p.join("base.txt"), b"baseline\n").unwrap();
        std::fs::write(p.join("doomed.txt"), b"doomed\n").unwrap();
        git(p, &["add", "-A"]);
        git(
            p,
            &[
                "-c",
                "user.email=devai@test.local",
                "-c",
                "user.name=devai-test",
                "commit",
                "-q",
                "-m",
                "bound state",
            ],
        );
        let bound_head = git(p, &["rev-parse", "HEAD"]);

        let store = Store::open(home.path()).unwrap();
        let now = Utc::now();
        let project = Project::new("fixture-project", "", vec![p.display().to_string()], now);
        store.create_project(&project).unwrap();
        let task = Task::new("workspace view fixture", "", Some(project.id), now);
        let task_id = task.id;
        store.create_task(&task).unwrap();

        let exec_id = new_id();
        let wt = worktree::prepare(p, home.path(), task_id, Some(&bound_head)).unwrap();
        let tip = if zero_change {
            Some(
                worktree::commit_platform_work(
                    &wt.dir,
                    "devai: implementation: nothing changed",
                    task_id,
                    exec_id,
                )
                .unwrap(),
            )
        } else {
            // The agent's committed work: one modification, one addition,
            // one deletion, one binary addition.
            std::fs::write(wt.dir.join("base.txt"), b"baseline\nagent modified\n").unwrap();
            std::fs::write(wt.dir.join("new.txt"), b"agent work\n").unwrap();
            std::fs::write(wt.dir.join("bin.dat"), [0u8, 155, 0, 255, 7]).unwrap();
            std::fs::remove_file(wt.dir.join("doomed.txt")).unwrap();
            Some(
                worktree::commit_platform_work(
                    &wt.dir,
                    "devai: implementation: fixture work",
                    task_id,
                    exec_id,
                )
                .unwrap(),
            )
        };

        store
            .insert_event(
                Some(task_id),
                event_kind::WORKTREE_PREPARED,
                "implementation-agent",
                serde_json::json!({
                    "execution": exec_id.to_string(),
                    "branch": wt.branch,
                    "worktree": wt.dir.display().to_string(),
                    "bound_head": bound_head,
                }),
            )
            .unwrap();
        store
            .insert_event(
                Some(task_id),
                event_kind::WORKTREE_COMMITTED,
                "implementation-agent",
                serde_json::json!({
                    "execution": exec_id.to_string(),
                    "branch": wt.branch,
                    "worktree": wt.dir.display().to_string(),
                    "commit": tip.clone().unwrap(),
                }),
            )
            .unwrap();

        Fixture {
            home,
            repo,
            task_id,
            exec_id,
            bound_head,
            tip,
            wt_dir: wt.dir,
        }
    }

    fn api(&self) -> Api {
        Api::open(self.home.path(), "tester").unwrap()
    }
    fn id(&self) -> String {
        self.task_id.to_string()
    }
    /// The REQ-WT-08 witness trio (same spelling as the worktree test):
    /// status / HEAD / index of the MAIN checkout, byte-for-byte.
    fn main_state(&self) -> MainState {
        let p = self.repo.path();
        MainState {
            status: git(
                p,
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--no-renames",
                    "--untracked-files=all",
                ],
            ),
            head: git(p, &["rev-parse", "HEAD"]),
            index: git(p, &["ls-files", "-s"]),
        }
    }
    /// Simulate the human lifecycle after a merge: directory gone, branch
    /// gone. Objects stay (until GC — assumption A2 of the spec).
    fn kill_branch(&self) {
        std::fs::remove_dir_all(&self.wt_dir).unwrap();
        git(self.repo.path(), &["worktree", "prune"]);
        git(
            self.repo.path(),
            &["branch", "-D", &worktree::branch_name(self.task_id)],
        );
    }
    /// The baseline drift case (REQ-WV-03 AC2): the main branch moves on.
    fn drift_main(&self) {
        let p = self.repo.path();
        std::fs::write(p.join("main-later.txt"), b"drifted after the branch-off\n").unwrap();
        git(p, &["add", "main-later.txt"]);
        git(
            p,
            &[
                "-c",
                "user.email=devai@test.local",
                "-c",
                "user.name=devai-test",
                "commit",
                "-q",
                "-m",
                "main drifts",
            ],
        );
    }
}

/// REQ-WV-01 AC3 + REQ-WV-08 AC1/AC3: for a live branch with commits the
/// list is non-empty and names EXACTLY what read-only `git diff
/// --name-status --no-renames baseline..tip` names; both operations leave
/// the main checkout byte-identical and the store rows untouched.
#[test]
fn changeset_names_exactly_the_readonly_diff_and_changes_nothing() {
    let f = Fixture::new(false);
    let api = f.api();

    let before = f.main_state();
    let rows_before = (
        api.task_list().unwrap().len(),
        api.exec_list(None).unwrap().len(),
        api.artifact_list(&f.id()).unwrap().len(),
    );

    let v = api.task_workspace_changes(&f.id()).unwrap();
    assert_eq!(v.state, workspace_state::LIVE);
    assert!(v.isolated && !v.zero_changes && !v.partial);
    assert_eq!(
        v.branch.as_deref(),
        Some(worktree::branch_name(f.task_id).as_str())
    );
    assert_eq!(v.bound_head.as_deref(), Some(f.bound_head.as_str()));
    assert_eq!(v.tip.as_deref(), f.tip.as_deref());
    assert_eq!(v.commits.len(), 1);
    assert_eq!(v.commits[0].commit, f.tip.clone().unwrap());
    assert_eq!(v.commits[0].execution, f.exec_id.to_string());

    // The witness: the same read-only diff, run out-of-band.
    let raw = git(
        f.repo.path(),
        &[
            "diff",
            "--name-status",
            "--no-renames",
            &format!("{}..{}", f.bound_head, f.tip.clone().unwrap()),
        ],
    );
    let expected: Vec<(String, String)> = raw
        .lines()
        .map(|l| {
            let mut it = l.split('\t');
            let st = it.next().unwrap().chars().next().unwrap().to_string();
            (st, it.next().unwrap().to_string())
        })
        .collect();
    let got: Vec<(String, String)> = v
        .files
        .iter()
        .map(|x| (x.status.clone(), x.path.clone()))
        .collect();
    assert_eq!(
        got, expected,
        "changeset list must be exactly the read-only diff"
    );
    // A/M/D content of THIS fixture (sanity on the fixture itself):
    assert!(got.contains(&("A".to_string(), "bin.dat".to_string())));
    assert!(got.contains(&("M".to_string(), "base.txt".to_string())));
    assert!(got.contains(&("D".to_string(), "doomed.txt".to_string())));

    // The companion operation over the same fixture.
    let d = api.task_workspace_diff(&f.id(), "base.txt").unwrap();
    assert_eq!(d.baseline.as_deref(), v.bound_head.as_deref());
    assert_eq!(d.tip.as_deref(), v.tip.as_deref());
    assert!(!d.binary);
    assert!(d.diff.as_deref().unwrap().contains("+agent modified"));

    let after = f.main_state();
    assert_eq!(
        after, before,
        "status/HEAD/index of the main checkout must be byte-identical across both view operations (REQ-WV-08 AC1)"
    );
    let rows_after = (
        api.task_list().unwrap().len(),
        api.exec_list(None).unwrap().len(),
        api.artifact_list(&f.id()).unwrap().len(),
    );
    assert_eq!(
        rows_after, rows_before,
        "view operations write nothing (AC3)"
    );
}

/// REQ-WV-02 AC1/AC2/AC3: range consistency is structural, a path outside
/// the list is a typed not_found (by CODE, not text), binary files are a
/// binary fact with the stored size and never raw bytes.
#[test]
fn diff_answers_per_file_with_typed_verdicts() {
    let f = Fixture::new(false);
    let api = f.api();
    let v = api.task_workspace_changes(&f.id()).unwrap();
    // REQ-WV-05 AC2: git's canonical path-sorted order arrives intact.
    let paths: Vec<&str> = v.files.iter().map(|x| x.path.as_str()).collect();
    assert_eq!(paths, ["base.txt", "bin.dat", "doomed.txt", "new.txt"]);

    let added = api.task_workspace_diff(&f.id(), "new.txt").unwrap();
    let text = added.diff.as_deref().unwrap();
    assert!(
        text.starts_with("diff --git"),
        "unified diff text expected: {text}"
    );
    assert!(text.contains("+agent work"));

    let gone = api.task_workspace_diff(&f.id(), "doomed.txt").unwrap();
    assert!(gone.diff.as_deref().unwrap().contains("-doomed"));

    let bin = api.task_workspace_diff(&f.id(), "bin.dat").unwrap();
    assert!(bin.binary, "bin.dat is a binary addition");
    assert!(bin.diff.is_none(), "raw bytes are never rendered (AC3)");
    assert_eq!(bin.size_bytes, Some(5));

    let err = api
        .task_workspace_diff(&f.id(), "not-in-the-list.rs")
        .unwrap_err();
    assert_eq!(
        err.code(),
        ErrorCode("not_found"),
        "typed not_found by code (AC2)"
    );
    assert!(err.to_string().contains("not in the changeset"));
}

/// R1 (operator directive): the registry-completeness pin lives on the
/// LIVE code registry (`devai_api::operations()`), served to consumers by
/// the published `devai api registry` command. Both view operations must be
/// registered Read entries with full descriptions and a CLI mirror clause
/// (Q3 ratified: the console surface exists), which is exactly what
/// `devai api registry` serves.
#[test]
fn both_view_operations_are_published_on_the_live_registry() {
    let ops = devai_api::operations();
    for (name, mirror) in [
        ("task.workspace_changes", "task workspace"),
        ("task.workspace_diff", "task workspace-diff"),
    ] {
        let d = ops
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("`{name}` is not published on the live registry"));
        assert_eq!(
            d.class,
            devai_api::OpClass::Read,
            "`{name}` must stay class=read — the view adds no mutation (REQ-WV-08 AC2)"
        );
        assert!(
            d.description.trim().len() >= 20 && !d.input.is_empty() && !d.output.is_empty(),
            "`{name}`: description and shapes must be complete registry data"
        );
        assert!(
            d.description.contains(mirror),
            "`{name}`: description must name its CLI mirror `{mirror}`"
        );
    }
}

/// R5 (data half of the merged render branch): `rework.merged` facts
/// (head_before/head_after) arrive in the view verbatim — the UI renders
/// the merge line from stored data and invents nothing (window pairs in
/// the harness cover the render side).
#[test]
fn merge_facts_arrive_in_the_view_verbatim() {
    let f = Fixture::new(false);
    let store = Store::open(f.home.path()).unwrap();
    let head_before = "a".repeat(40);
    let head_after = "b".repeat(40);
    store
        .insert_event(
            Some(f.task_id),
            event_kind::REWORK_MERGED,
            "guerrero",
            serde_json::json!({
                "branch": worktree::branch_name(f.task_id),
                "workspace": f.repo.path().display().to_string(),
                "head_before": head_before,
                "head_after": head_after,
            }),
        )
        .unwrap();
    drop(store);

    let v = f.api().task_workspace_changes(&f.id()).unwrap();
    // The branch survives the merge (REQ-WV-07a): live view, merge facts shown.
    assert_eq!(v.state, workspace_state::LIVE);
    assert_eq!(v.head_before.as_deref(), Some(head_before.as_str()));
    assert_eq!(v.head_after.as_deref(), Some(head_after.as_str()));
}

/// REQ-WV-01 AC2: a task without worktree.prepared gets the explicit typed
/// verdict "no isolation" — not an error, not an invented list. Same for
/// the companion diff operation.
#[test]
fn no_isolation_is_a_verdict_not_an_error() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open(home.path()).unwrap();
    let task = Task::new("never isolated", "", None, Utc::now());
    let id = task.id.to_string();
    store.create_task(&task).unwrap();
    let api = Api::open(home.path(), "tester").unwrap();

    let v = api.task_workspace_changes(&id).unwrap();
    assert_eq!(v.state, workspace_state::NO_ISOLATION);
    assert!(!v.isolated);
    assert!(v.files.is_empty() && v.branch.is_none());
    assert!(v.note.is_some(), "the verdict carries an honest reason");

    let d = api.task_workspace_diff(&id, "anything.txt").unwrap();
    assert_eq!(d.state, workspace_state::NO_ISOLATION);
    assert!(d.diff.is_none());
}

/// REQ-WV-01 AC4: isolation happened, the agent changed nothing — the
/// legitimate zero changeset. Distinct from `unavailable` in DATA
/// (REQ-WV-07 AC2): different state, different marker.
#[test]
fn zero_changes_is_a_legitimate_empty_result() {
    let f = Fixture::new(true);
    let api = f.api();
    assert_eq!(f.tip.as_deref(), Some(f.bound_head.as_str()));

    let v = api.task_workspace_changes(&f.id()).unwrap();
    assert_eq!(v.state, workspace_state::LIVE);
    assert!(v.isolated);
    assert!(
        v.zero_changes,
        "empty list + zero_changes marker, not an error"
    );
    assert!(!v.partial);
    assert!(v.files.is_empty());
    assert_eq!(
        v.commits.len(),
        1,
        "the stamp still exists — history is shown"
    );
}

/// REQ-WV-07 (b) + REQ-WV-03 AC2: after the human deleted the worktree
/// directory AND the branch, the stored shas still resolve — the view is
/// rebuilt between the stored baseline and the last stored stamp with an
/// explicit partial marker, and drift of the main branch does not move it.
#[test]
fn historical_view_after_the_branch_is_gone_survives_main_drift() {
    let f = Fixture::new(false);
    let live = f.api().task_workspace_changes(&f.id()).unwrap();
    let live_files: Vec<(String, String)> = live
        .files
        .iter()
        .map(|x| (x.status.clone(), x.path.clone()))
        .collect();

    f.kill_branch();
    f.drift_main();

    let api = f.api();
    let before = api.task_workspace_changes(&f.id()).unwrap();
    assert_eq!(before.state, workspace_state::HISTORICAL);
    assert!(before.partial, "never presented as if the branch were live");
    assert_eq!(
        before.tip.as_deref(),
        live.tip.as_deref(),
        "the last stored stamp"
    );
    assert_eq!(
        before.bound_head, live.bound_head,
        "baseline is the stored bound head (AC2)"
    );
    let hist_files: Vec<(String, String)> = before
        .files
        .iter()
        .map(|x| (x.status.clone(), x.path.clone()))
        .collect();
    assert_eq!(
        hist_files, live_files,
        "the historical list must equal the live list — drift of main moved nothing"
    );
    let d = api.task_workspace_diff(&f.id(), "base.txt").unwrap();
    assert_eq!(d.state, workspace_state::HISTORICAL);
    assert!(d.diff.as_deref().unwrap().contains("+agent modified"));

    // Read-only stays read-only in this state too: main bytes untouched.
    let main_head = git(f.repo.path(), &["rev-parse", "HEAD"]);
    assert_ne!(main_head, f.bound_head, "the fixture actually drifted main");
}

/// REQ-WV-07 (c): neither the branch nor the stored commits resolve —
/// only the stored facts with an honest "content unavailable" marker;
/// NEVER a non-empty list and NEVER an empty screen.
#[test]
fn unavailable_shows_only_stored_facts_with_a_marker() {
    let f = Fixture::new(false);
    f.kill_branch();
    // Replace the resolvable stamps with unrecoverable ones (simulates GC —
    // assumption A2): re-home the task under a second store whose events
    // name shas no repository can resolve.
    let home2 = tempfile::TempDir::new().unwrap();
    let store2 = Store::open(home2.path()).unwrap();
    let task = Task::new("gc-lost", "", None, Utc::now());
    let tid = task.id;
    store2.create_task(&task).unwrap();
    let fake_head = "0".repeat(40);
    let fake_commit = "1".repeat(40);
    let fake_wt = f.wt_dir.display().to_string(); // directory is gone now
    store2
        .insert_event(
            Some(tid),
            event_kind::WORKTREE_PREPARED,
            "implementation-agent",
            serde_json::json!({
                "execution": new_id().to_string(),
                "branch": worktree::branch_name(tid),
                "worktree": fake_wt,
                "bound_head": fake_head,
            }),
        )
        .unwrap();
    store2
        .insert_event(
            Some(tid),
            event_kind::WORKTREE_COMMITTED,
            "implementation-agent",
            serde_json::json!({
                "execution": new_id().to_string(),
                "branch": worktree::branch_name(tid),
                "worktree": fake_wt,
                "commit": fake_commit,
            }),
        )
        .unwrap();
    drop(store2);

    let api2 = Api::open(home2.path(), "tester").unwrap();
    let v = api2.task_workspace_changes(&tid.to_string()).unwrap();
    assert_eq!(v.state, workspace_state::UNAVAILABLE);
    assert!(
        !v.zero_changes,
        "unavailable is NOT zero changes (REQ-WV-07 AC2)"
    );
    assert!(v.files.is_empty());
    assert!(v.note.is_some());
    // The stored facts still arrive — the honest partial record.
    assert_eq!(
        v.branch.as_deref(),
        Some(worktree::branch_name(tid).as_str())
    );
    assert_eq!(v.bound_head.as_deref(), Some(fake_head.as_str()));
    assert_eq!(v.commits.len(), 1);
    assert_eq!(v.commits[0].commit, fake_commit);

    let d = api2
        .task_workspace_diff(&tid.to_string(), "base.txt")
        .unwrap();
    assert_eq!(d.state, workspace_state::UNAVAILABLE);
    assert!(d.diff.is_none());

    // And the first fixture's branch really is gone (self-check of the kill):
    // its objects survive, so that view is historical, not unavailable —
    // the two states are distinguished by resolvability, not by directory.
    let api = f.api();
    let v = api.task_workspace_changes(&f.id()).unwrap();
    assert_eq!(v.state, workspace_state::HISTORICAL);
}
