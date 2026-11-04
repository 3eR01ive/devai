//! Cycle 2 (REQ-MUT-06): output parity of the mutation plane. These tests
//! pin the EXACT operator-facing texts of the commands whose store access
//! moved behind the API: creation with edges, the dependency commands, the
//! workflow render lines, the human-gate refusals and the contract-gate
//! presentation. The pre-cycle-2 behavior is the baseline — the same
//! sentences must appear from the same commands, only now they arrive as
//! machine-coded API errors rendered by the shared path (REQ-MUT-08).
//! The existing suites (single_writer, empty_rework_gate, merge_gate,
//! dependencies) keep pinning the deeper gate semantics end-to-end.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn devai(home: &Path, args: &[&str]) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("parity-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    Out {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn ok(home: &Path, args: &[&str]) -> String {
    let o = devai(home, args);
    assert!(o.code == Some(0), "devai {args:?} exited {:?}\nstdout: {}\nstderr: {}", o.code, o.stdout, o.stderr);
    o.stdout
}

fn fail(home: &Path, args: &[&str]) -> (String, String) {
    let o = devai(home, args);
    assert!(o.code != Some(0), "devai {args:?} unexpectedly succeeded:\n{}", o.stdout);
    (o.stdout, o.stderr)
}

/// The short id of the `task new` line (the print starts `task <id8> …`).
fn new_id_of(stdout: &str) -> String {
    stdout["task ".len()..].split_whitespace().next().unwrap().to_string()
}

/// The `op()` dispatch render lines the CLI printed for every workflow
/// mutation — recreated by the API `MutationResult` path verbatim.
#[test]
fn workflow_mutation_render_parity() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    let out = ok(home, &["task", "new", "parity task"]);
    assert!(out.starts_with("task ") && out.contains(" created in DRAFT — workflow is owned by devai\n"), "{out}");
    assert!(out.contains("start it with: devai task start "), "{out}");
    let id = new_id_of(&out);

    let out = ok(home, &["task", "start", &id]);
    assert!(out.contains("state: DRAFT → ANALYSIS\n"), "{out}");
    assert!(out.contains("next: "), "{out}");

    // Pause/resume keep the transition form; a wrong-state review keeps
    // the core guard refusal text — now an API error with the same Display.
    let out = ok(home, &["task", "pause", &id, "--reason", "parity"]);
    assert!(out.contains("state: ANALYSIS → PAUSED\n"), "{out}");
    let out = ok(home, &["task", "resume", &id]);
    assert!(out.contains("state: PAUSED → ANALYSIS\n"), "{out}");

    let (so, se) = fail(home, &["task", "review", &id, "--verdict", "nope"]);
    assert!(se.contains("devai: unknown verdict `nope` (approved|needs_revision|rejected|returned)"), "{se} | {so}");
    assert!(so.is_empty(), "stdout must stay clean on refusal: {so}");

    // Approving outside APPROVAL refuses with the core guard sentence.
    let (_so, se) = fail(home, &["task", "approve", &id, "--rationale", "too early"]);
    assert!(se.contains("approve"), "{se}");
}

/// `task new` genealogy edges, `task depend` / `task unlink` lines — the
/// texts the direct-store handlers printed, now rendered from typed API
/// results. The unlink line's actor is the SUPERSEDER (the edge row keeps
/// its original author — `UnlinkResult.superseded_by` carries the new one).
#[test]
fn dependency_edges_render_and_refusal_parity() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    let a = ok(home, &["task", "new", "dep-a"]);
    let aid = new_id_of(&a);
    let b = ok(home, &["task", "new", "dep-b", "--blocked-by", &aid]);
    assert!(b.contains(&format!("  depends blocks {aid} (DRAFT in dep-a)")), "{b}");

    // Self-reference stays loud (unchanged rule, unchanged text family).
    let (so, se) = fail(home, &["task", "depend", &aid, "--on", &aid, "--kind", "parent"]);
    assert!(se.contains("invalid task dependency"), "{se}");
    assert!(so.is_empty(), "{so}");

    // Duplicate registration is loud at write too.
    let (_so, se) = fail(home, &["task", "depend", &b.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_string(), "--on", &aid, "--kind", "blocks"]);
    assert!(se.contains("already exists"), "{se}");

    let out = ok(home, &["task", "depend", &aid, "--on", &b.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_string(), "--kind", "parent"]);
    let bid = b.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_string();
    let line = format!("task {aid} —parent→ task {bid} recorded (DRAFT in dep-b, by parity-human, ");
    assert!(out.starts_with(&line), "expected `{line}`…\ngot: {out}");

    let out = ok(home, &["task", "unlink", &aid, "--on", &bid, "--kind", "parent"]);
    assert!(out.contains("superseded (edge ") && out.ends_with("stays as history, by parity-human)\n"), "{out}");

    // Override with nothing to override keeps the refusal sentence
    // (dep-a has no outgoing blockers after the unlink above).
    let (_so, se) = fail(home, &["task", "override", &aid]);
    assert!(se.contains(&format!("task {aid} has no open blocking dependencies — nothing to override")), "{se}");
}

/// The human-gate refusals that used to be CLI-internal `bail!` texts are
/// now machine-coded API errors with the identical Display (REQ-MUT-08):
/// the ratify gate applicability check and the execution-row close check.
#[test]
fn gate_refusal_texts_survive_the_move_to_api_errors() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    let a = ok(home, &["task", "new", "gate-refusal"]);
    let aid = new_id_of(&a);

    let (_so, se) = fail(home, &["task", "ratify-empty-rework", &aid]);
    assert_eq!(se.trim(), "devai: ratify-empty-rework applies to the empty-rework gate (REWORK, or WAITING_FOR_INPUT returning to REWORK); the task is in DRAFT");

    // exec start in DRAFT refuses through the core role/stage matrix…
    let (_so, se) = fail(home, &["exec", "start", &aid, "--role", "quality-auditor"]);
    assert!(se.contains("run-role-execution"), "{se}");
    // …with a legal role it starts; double-close then refuses with the
    // exact old sentence (full ids in text, as the pre-API handler had).
    ok(home, &["task", "start", &aid]);
    ok(home, &["exec", "start", &aid, "--role", "analyst"]);
    let listing = ok(home, &["--json", "exec", "list", "--task", &aid]);
    let v: serde_json::Value = serde_json::from_str(&listing).unwrap();
    let eid = v[0]["id"].as_str().unwrap().to_string();
    let out = ok(home, &["exec", "finish", &eid, "--summary", "done"]);
    assert!(out.starts_with(&format!("execution {} completed", devai_core::types::short_id_str(&eid))), "{out}");
    let (_so, se) = fail(home, &["exec", "finish", &eid, "--summary", "again"]);
    assert_eq!(se.trim(), format!("devai: execution {eid} is COMPLETED, not RUNNING"));
}

/// The contract gate (ratified OQ-FG-05): the API refuses with the machine
/// list, the CLI renders the ratified PRESENTATION — one warning per
/// violation on stderr, then the refusal with the full list, and a nonzero
/// exit. Nothing is stored and nothing prints before the gate (same as the
/// pre-API publish_raw). The separate phase order is pinned too: an
/// ADVANCE failure happens after the row is published and printed — the
/// two API operations keep that print-then-effect sequence.
#[test]
fn contract_gate_presentation_parity() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);

    let schema_path = home.join("contract.json");
    std::fs::write(&schema_path, serde_json::json!({"type": "object", "required": ["must_exist"]}).to_string()).unwrap();
    let out = ok(home, &["contract", "add", "--name", "parity-contract", "--version", "1", "--file", schema_path.to_str().unwrap()]);
    assert_eq!(out, "contract parity-contract@1 registered\n");

    let body = home.join("body.json");
    std::fs::write(&body, serde_json::json!({"nope": 1}).to_string()).unwrap();

    // Task ref resolves BEFORE the file read (old order): a dangling ref
    // is reported as the store's NotFound sentence even with a bad path.
    let bad_body = home.join("missing.json");
    let (_so, se) = fail(home, &["artifact", "publish", "no-such-task", "--kind", "test_report", "--file", bad_body.to_str().unwrap()]);
    assert!(se.starts_with("devai: task `no-such-task` not found"), "{se}");

    let a = ok(home, &["task", "new", "contract-gate"]);
    let aid = new_id_of(&a);
    let (so, se) = fail(home, &["artifact", "publish", &aid, "--kind", "test_report", "--file", body.to_str().unwrap(), "--contract", "parity-contract@1"]);
    assert!(so.is_empty(), "the contract gate refuses before any print/storage: {so}");
    let warnings: Vec<&str> = se.lines().filter(|l| l.starts_with("warning: parity-contract@1 contract gate rejected this artifact: ")).collect();
    assert!(!warnings.is_empty(), "{se}");
    let last = se.lines().last().unwrap();
    assert!(last.starts_with("devai: artifact does not satisfy contract parity-contract@1: ["), "{last}");

    // Unregistered contract — the same sentence as the pre-API bail.
    let (_so, se) = fail(home, &["artifact", "publish", &aid, "--kind", "test_report", "--file", body.to_str().unwrap(), "--contract", "ghost@2"]);
    assert_eq!(se.trim(), "devai: contract ghost@2 is not registered (devai contract add)");

    // Phase order: the row lands and PRINTS, then the advance refuses
    // (requirements publication in DRAFT) — exactly the pre-cycle-2
    // interleaving, preserved by the phased operations.
    let spec = home.join("spec.json");
    std::fs::write(&spec, serde_json::json!({"body": "draft spec"}).to_string()).unwrap();
    let (so, se) = fail(home, &["artifact", "publish", &aid, "--kind", "requirements_spec", "--file", spec.to_str().unwrap()]);
    assert!(so.starts_with("artifact REQUIREMENTS_SPEC "), "{so}");
    assert!(se.contains("run `devai task start` first"), "{se}");
}

/// The registry CLI view covers the mutation plane (REQ-MUT-05 AC2) and
/// never lists the bootstrap pair (OQ-10 stays outside the surface).
#[test]
fn registry_output_covers_mutations() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    let out = ok(home, &["api", "registry"]);
    for name in ["task.approve", "task.create", "artifact.publish", "exec.start", "question.answer", "task.merge"] {
        assert!(out.contains(&format!("{name}  [mutation]")), "{name} missing from the registry output");
    }
    assert!(!out.contains("init  ["), "init leaked into the registry");
    assert!(!out.contains("seed  ["), "seed leaked into the registry");
    assert!(out.contains("operations published"), "footer line missing");
}

/// REQ-MUT-02 AC1 / REQ-MUT-03 / human-ratified Q2 — the total prohibition:
/// the CLI sources contain no store handle at all. No `ctx.store`, no
/// `devai_store` import, no `Store::open` by name, no `Api::store()`/
/// `Api::new`. (Tests are a separate surface: internal crate access to the
/// store stays allowed by the spec assumption — only `src/` is pinned.)
#[test]
fn cli_sources_touch_no_store() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("main.rs"), src.join("commands.rs"), src.join("out.rs")];
    let mut violations: Vec<String> = Vec::new();
    while let Some(p) = files.pop() {
        if p.is_dir() {
            for e in std::fs::read_dir(&p).unwrap() {
                files.push(e.unwrap().path());
            }
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for needle in ["ctx.store", "devai_store", "Store::open", "Api::new", ".store()"] {
            if text.contains(needle) {
                violations.push(format!("{}: `{needle}`", p.display()));
            }
        }
    }
    assert!(violations.is_empty(), "CLI sources reach the store again: {violations:?}");
}
