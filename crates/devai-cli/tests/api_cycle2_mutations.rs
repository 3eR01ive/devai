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
    assert!(
        o.code == Some(0),
        "devai {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        o.code,
        o.stdout,
        o.stderr
    );
    o.stdout
}

fn fail(home: &Path, args: &[&str]) -> (String, String) {
    let o = devai(home, args);
    assert!(
        o.code != Some(0),
        "devai {args:?} unexpectedly succeeded:\n{}",
        o.stdout
    );
    (o.stdout, o.stderr)
}

/// The FULL uuid of a freshly created task, taken from the machine-readable
/// `--json` view (REQ-E2E-01: cross-step references travel as full ids,
/// never as human-stdout truncations — UUIDv7 display shorts collide inside
/// one millisecond window, and that flake is what amplified into the QA
/// 01a1acab-18b1 orphan finding). The human render itself stays unchanged:
/// stdout still prints the canonical display short, and assertions compare
/// it through the canonical accessor, never through a test-local width.
fn full_id_of(home: &Path, title: &str) -> String {
    let listing = ok(home, &["--json", "task", "list"]);
    let v: serde_json::Value = serde_json::from_str(&listing).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"].as_str() == Some(title))
        .unwrap_or_else(|| panic!("task `{title}` missing from `task list --json`:\n{listing}"))
        ["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The `op()` dispatch render lines the CLI printed for every workflow
/// mutation — recreated by the API `MutationResult` path verbatim.
#[test]
fn workflow_mutation_render_parity() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    let out = ok(home, &["task", "new", "parity task"]);
    assert!(
        out.starts_with("task ")
            && out.contains(" created in DRAFT — workflow is owned by devai\n"),
        "{out}"
    );
    assert!(out.contains("start it with: devai task start "), "{out}");
    let id = full_id_of(home, "parity task");

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
    assert!(
        se.contains("devai: unknown verdict `nope` (approved|needs_revision|rejected|returned)"),
        "{se} | {so}"
    );
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
    ok(home, &["task", "new", "dep-a"]);
    let aid = full_id_of(home, "dep-a");
    // REQ-E2E-01: the cross-step reference travels as the FULL uuid from
    // --json — the flaky 13-char stdout short (a fresh collision candidate
    // in every same-millisecond window) is gone from this pass.
    let b = ok(home, &["task", "new", "dep-b", "--blocked-by", &aid]);
    assert!(
        b.contains(&format!(
            "  depends blocks {} (DRAFT in dep-a)",
            devai_core::types::short_id_str(&aid)
        )),
        "{b}"
    );
    let bid = full_id_of(home, "dep-b");

    // Self-reference stays loud (unchanged rule, unchanged text family).
    let (so, se) = fail(
        home,
        &["task", "depend", &aid, "--on", &aid, "--kind", "parent"],
    );
    assert!(se.contains("invalid task dependency"), "{se}");
    assert!(so.is_empty(), "{so}");

    // Duplicate registration is loud at write too.
    let (_so, se) = fail(
        home,
        &["task", "depend", &bid, "--on", &aid, "--kind", "blocks"],
    );
    assert!(se.contains("already exists"), "{se}");

    let out = ok(
        home,
        &["task", "depend", &aid, "--on", &bid, "--kind", "parent"],
    );
    // The human line still PRINTS the canonical display shorts — asserted
    // through the shared accessor, not a test-local width (REQ-E2E-02 AC3).
    let line = format!(
        "task {} —parent→ task {} recorded (DRAFT in dep-b, by parity-human, ",
        devai_core::types::short_id_str(&aid),
        devai_core::types::short_id_str(&bid)
    );
    assert!(out.starts_with(&line), "expected `{line}`…\ngot: {out}");

    let out = ok(
        home,
        &["task", "unlink", &aid, "--on", &bid, "--kind", "parent"],
    );
    assert!(
        out.contains("superseded (edge ") && out.ends_with("stays as history, by parity-human)\n"),
        "{out}"
    );

    // Override with nothing to override keeps the refusal sentence
    // (dep-a has no outgoing blockers after the unlink above). The printed
    // id in that sentence is the display short — compared canonically.
    let (_so, se) = fail(home, &["task", "override", &aid]);
    assert!(
        se.contains(&format!(
            "task {} has no open blocking dependencies — nothing to override",
            devai_core::types::short_id_str(&aid)
        )),
        "{se}"
    );
}

/// The human-gate refusals that used to be CLI-internal `bail!` texts are
/// now machine-coded API errors with the identical Display (REQ-MUT-08):
/// the ratify gate applicability check and the execution-row close check.
#[test]
fn gate_refusal_texts_survive_the_move_to_api_errors() {
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    ok(home, &["task", "new", "gate-refusal"]);
    let aid = full_id_of(home, "gate-refusal");

    let (_so, se) = fail(home, &["task", "ratify-empty-rework", &aid]);
    assert_eq!(
        se.trim(),
        "devai: ratify-empty-rework applies to the empty-rework gate (REWORK, or WAITING_FOR_INPUT returning to REWORK); the task is in DRAFT"
    );

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
    assert!(
        out.starts_with(&format!(
            "execution {} completed",
            devai_core::types::short_id_str(&eid)
        )),
        "{out}"
    );
    let (_so, se) = fail(home, &["exec", "finish", &eid, "--summary", "again"]);
    assert_eq!(
        se.trim(),
        format!("devai: execution {eid} is COMPLETED, not RUNNING")
    );
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
    std::fs::write(
        &schema_path,
        serde_json::json!({"type": "object", "required": ["must_exist"]}).to_string(),
    )
    .unwrap();
    let out = ok(
        home,
        &[
            "contract",
            "add",
            "--name",
            "parity-contract",
            "--version",
            "1",
            "--file",
            schema_path.to_str().unwrap(),
        ],
    );
    assert_eq!(out, "contract parity-contract@1 registered\n");

    let body = home.join("body.json");
    std::fs::write(&body, serde_json::json!({"nope": 1}).to_string()).unwrap();

    // Task ref resolves BEFORE the file read (old order): a dangling ref
    // is reported as the store's NotFound sentence even with a bad path.
    let bad_body = home.join("missing.json");
    let (_so, se) = fail(
        home,
        &[
            "artifact",
            "publish",
            "no-such-task",
            "--kind",
            "test_report",
            "--file",
            bad_body.to_str().unwrap(),
        ],
    );
    assert!(
        se.starts_with("devai: task `no-such-task` not found"),
        "{se}"
    );

    ok(home, &["task", "new", "contract-gate"]);
    let aid = full_id_of(home, "contract-gate");
    let (so, se) = fail(
        home,
        &[
            "artifact",
            "publish",
            &aid,
            "--kind",
            "test_report",
            "--file",
            body.to_str().unwrap(),
            "--contract",
            "parity-contract@1",
        ],
    );
    assert!(
        so.is_empty(),
        "the contract gate refuses before any print/storage: {so}"
    );
    let warnings: Vec<&str> = se
        .lines()
        .filter(|l| {
            l.starts_with("warning: parity-contract@1 contract gate rejected this artifact: ")
        })
        .collect();
    assert!(!warnings.is_empty(), "{se}");
    let last = se.lines().last().unwrap();
    assert!(
        last.starts_with("devai: artifact does not satisfy contract parity-contract@1: ["),
        "{last}"
    );

    // Unregistered contract — the same sentence as the pre-API bail.
    let (_so, se) = fail(
        home,
        &[
            "artifact",
            "publish",
            &aid,
            "--kind",
            "test_report",
            "--file",
            body.to_str().unwrap(),
            "--contract",
            "ghost@2",
        ],
    );
    assert_eq!(
        se.trim(),
        "devai: contract ghost@2 is not registered (devai contract add)"
    );

    // Phase order: the row lands and PRINTS, then the advance refuses
    // (requirements publication in DRAFT) — exactly the pre-cycle-2
    // interleaving, preserved by the phased operations.
    let spec = home.join("spec.json");
    std::fs::write(&spec, serde_json::json!({"body": "draft spec"}).to_string()).unwrap();
    let (so, se) = fail(
        home,
        &[
            "artifact",
            "publish",
            &aid,
            "--kind",
            "requirements_spec",
            "--file",
            spec.to_str().unwrap(),
        ],
    );
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
    for name in [
        "task.approve",
        "task.create",
        "artifact.publish",
        "exec.start",
        "question.answer",
        "task.merge",
    ] {
        assert!(
            out.contains(&format!("{name}  [mutation]")),
            "{name} missing from the registry output"
        );
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
    let mut files = vec![
        src.join("main.rs"),
        src.join("commands.rs"),
        src.join("out.rs"),
    ];
    let mut violations: Vec<String> = Vec::new();
    while let Some(p) = files.pop() {
        if p.is_dir() {
            for e in std::fs::read_dir(&p).unwrap() {
                files.push(e.unwrap().path());
            }
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for needle in [
            "ctx.store",
            "devai_store",
            "Store::open",
            "Api::new",
            ".store()",
        ] {
            if text.contains(needle) {
                violations.push(format!("{}: `{needle}`", p.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "CLI sources reach the store again: {violations:?}"
    );
}

/// REQ-API-01 regression at the CLI surface (QA artifact 01a1acab-18b1,
/// rl/idempotency): `task new` with an AMBIGUOUS blocked_by ref refuses —
/// and a DIRECT store read proves neither the task nor any of its edges
/// landed. Pre-fix, the task was inserted BEFORE the refs resolved, so this
/// exact call left an orphan task plus an already-committed parent edge.
#[test]
fn task_new_with_ambiguous_ref_leaves_no_orphan() {
    use devai_core::types::Task;
    use devai_store::Store;
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    // A genuine ambiguity, crafted without wall-clock luck: two rows whose
    // ids share the FULL canonical 13-char display prefix.
    let mut a = Task::new("collision-a", String::new(), None, chrono::Utc::now());
    a.id = uuid::Uuid::parse_str("019e0000-0000-7000-8000-0000000000aa").unwrap();
    let mut b = Task::new("collision-b", String::new(), None, chrono::Utc::now());
    b.id = uuid::Uuid::parse_str("019e0000-0000-7000-8000-0000000000ab").unwrap();
    {
        let store = Store::open(home).unwrap();
        store.create_task(&a).unwrap();
        store.create_task(&b).unwrap();
    }

    let (so, se) = fail(
        home,
        &[
            "task",
            "new",
            "orphan-candidate",
            "--parent",
            &a.id.to_string(),
            "--blocked-by",
            "019e0000-0000", // matches BOTH crafted rows
        ],
    );
    assert!(se.contains("ambiguous identifier"), "{se}");
    assert!(so.is_empty(), "stdout must stay clean on refusal: {so}");

    // DIRECT read: nothing landed — not the task, not even the edge whose
    // ref DID resolve (the pre-fix partial success).
    let store = Store::open(home).unwrap();
    let tasks = store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 2, "no orphan task after refusal: {tasks:?}");
    assert!(store.incoming_dependencies(a.id).unwrap().is_empty());
    assert!(store.incoming_dependencies(b.id).unwrap().is_empty());

    // REQ-API-02 AC1: the identical retry with resolvable refs succeeds and
    // adds EXACTLY one task plus exactly its requested edge pair — the
    // refused attempt contributed nothing that could double.
    ok(home, &[
        "task",
        "new",
        "orphan-candidate",
        "--parent",
        &a.id.to_string(),
        "--blocked-by",
        &b.id.to_string(),
    ]);
    let store = Store::open(home).unwrap();
    let all = store.list_tasks().unwrap();
    assert_eq!(all.len(), 3);
    let child = all.iter().find(|t| t.title == "orphan-candidate").unwrap();
    assert_eq!(store.task_dependencies(child.id).unwrap().len(), 2);
}

/// REQ-API-02 / ratified OQ-1 at the CLI surface: repeating `task new` with
/// the SAME --idempotency-key returns the committed task — `--json` shows
/// the identical id and a DIRECT store read proves exactly one row. The
/// lost-response retry is thus a safe no-duplicate operation; keyless calls
/// keep the per-call-creates semantics.
#[test]
fn task_new_idempotency_key_retry_does_not_duplicate() {
    use devai_store::Store;
    let dir = TempDir::new().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    let first: serde_json::Value = serde_json::from_str(&ok(
        home,
        &["--json", "task", "new", "dup-guard", "--idempotency-key", "retry-42"],
    ))
    .unwrap();
    let again: serde_json::Value = serde_json::from_str(&ok(
        home,
        &["--json", "task", "new", "dup-guard", "--idempotency-key", "retry-42"],
    ))
    .unwrap();
    assert_eq!(first["id"], again["id"], "same key must return the SAME task");
    assert_eq!(
        Store::open(home)
            .unwrap()
            .list_tasks()
            .unwrap()
            .iter()
            .filter(|t| t.title == "dup-guard")
            .count(),
        1,
        "direct read: the retry created no duplicate"
    );

    // A different key is a different command — it creates.
    ok(home, &["--json", "task", "new", "dup-guard", "--idempotency-key", "retry-43"]);
    assert_eq!(
        Store::open(home)
            .unwrap()
            .list_tasks()
            .unwrap()
            .iter()
            .filter(|t| t.title == "dup-guard")
            .count(),
        2
    );
    // Keyless calls: the plain per-call semantics stay untouched.
    ok(home, &["task", "new", "dup-guard"]);
    assert_eq!(
        Store::open(home)
            .unwrap()
            .list_tasks()
            .unwrap()
            .iter()
            .filter(|t| t.title == "dup-guard")
            .count(),
        3
    );
}
