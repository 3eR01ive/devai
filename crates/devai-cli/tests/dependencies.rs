//! Task dependencies through the REAL binary: genealogy registered at
//! creation (`--parent`/`--blocked-by`), retroactively (`task depend`),
//! read back as stored data (`task dependencies`, `task graph`), and the
//! gates that refuse resume/acceptance while a `blocks` edge is open
//! (REQ-DEP-03) and let it through once closed.
//!
//! Ids are passed in FULL here: tasks created inside one clock millisecond
//! share their 12-char prefix (uuidv7 timestamp), and prefix resolution is
//! already covered by the progress suite — this suite pins the graph.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("dep-human")
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
    serde_json::from_str(s.trim())
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print one JSON object ({e}): {s}"))
}

/// A command expected to FAIL; returns its stderr.
fn devai_fails(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("dep-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "devai {args:?} must exit nonzero\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn id_of(v: &serde_json::Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

fn write(home: &Path, name: &str, content: &str) -> String {
    let p = home.join(name);
    std::fs::write(&p, content).unwrap();
    p.display().to_string()
}

#[test]
fn cli_links_dependencies_at_creation_and_gates_resume_on_open_blockers() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    // The business case verbatim, through the CLI: a parent spawns a derived
    // task; the "exists because" is recorded AT CREATION (REQ-DEP-07), not
    // in the operator chat.
    let a_id = id_of(&devai_json(
        h,
        &[
            "task",
            "new",
            "Backfill pause",
            "--description",
            "the parent",
        ],
    ));
    let created = devai(
        h,
        &[
            "task",
            "new",
            "Derived gate",
            "--description",
            "born from a",
            "--parent",
            &a_id,
            "--blocked-by",
            &a_id,
        ],
    );
    assert!(
        created.contains("depends parent") && created.contains("depends blocks"),
        "both edges named at creation:\n{created}"
    );
    let tasks = devai_json(h, &["task", "list"]);
    let b_id = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "Derived gate")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // REQ-DEP-01/04: the edge set reads back from stored data, typed, with
    // the depended-on task's state, attributed to the acting human.
    let deps = devai_json(h, &["task", "dependencies", &b_id]);
    let out = deps["outgoing"].as_array().unwrap();
    assert_eq!(
        out.len(),
        2,
        "parent + blocks on one pair, stored separately"
    );
    assert!(
        out.iter()
            .any(|d| d["kind"] == "PARENT" && d["depends_on_id"] == a_id.as_str())
    );
    assert!(
        out.iter()
            .any(|d| d["kind"] == "BLOCKS" && d["depends_on_id"] == a_id.as_str())
    );
    assert!(
        out.iter()
            .all(|d| d["depends_on_state"] == "DRAFT" && d["actor"] == "dep-human")
    );

    // REQ-DEP-06: no epic entity — the epic role is the SAME query from the
    // other side; only the 'parent' edges make it the root of a plan tree.
    let ad = devai_json(h, &["task", "dependencies", &a_id]);
    assert_eq!(
        ad["incoming"].as_array().unwrap().len(),
        2,
        "both edges visible from the parent side"
    );
    let at = devai(h, &["task", "dependencies", &a_id]);
    assert!(
        at.contains("epic role: 1 task(s)"),
        "epic reads out of the dependency view (parent edges only):\n{at}"
    );

    // REQ-DEP-04/05: the graph yields computable readiness from stored data.
    let g = devai_json(h, &["task", "graph"]);
    let ready: Vec<&str> = g["ready"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(
        ready.contains(&a_id.as_str()),
        "the unblocked parent is ready"
    );
    assert!(
        !ready.contains(&b_id.as_str()),
        "an open 'blocks' edge holds the child"
    );
    let gt = devai(h, &["task", "graph"]);
    assert!(
        gt.contains("open blockers:"),
        "text view names the hold:\n{gt}"
    );

    // REQ-DEP-03 AC1/AC2: resume refuses while the edge is open, and the
    // refusal enumerates the blocking task id AS DATA (exit nonzero).
    devai(h, &["task", "pause", &b_id, "--reason", "hold"]);
    let err = devai_fails(h, &["task", "resume", &b_id]);
    assert!(
        err.contains("open blocking dependencies") && err.contains(&a_id),
        "refusal names the blocker: {err}"
    );
    assert_eq!(
        devai_json(h, &["task", "get", &b_id])["state"],
        "PAUSED",
        "the refusal moved nothing"
    );

    // RATIFIED DECISION OQ4 at the CLI: a parent edge the OTHER way would
    // close a parent cycle — refused loudly, exit nonzero.
    let err = devai_fails(
        h,
        &["task", "depend", &a_id, "--on", &b_id, "--kind", "parent"],
    );
    assert!(err.contains("cycle"), "per-kind cycle refused: {err}");

    // RATIFIED DECISION OQ1: cancelling the blocker does NOT close the
    // edge — a dead dependency is an unresolved problem, resume still fails.
    devai(
        h,
        &[
            "task",
            "cancel",
            &a_id,
            "--reason",
            "cancelled, not resolved",
        ],
    );
    let err = devai_fails(h, &["task", "resume", &b_id]);
    assert!(
        err.contains("open blocking dependencies") && err.contains(&a_id),
        "CANCELLED does not close (OQ1): {err}"
    );

    // RATIFIED DECISION OQ2 — the human escape: `task override` records an
    // explicit dependency_override decision covering the OPEN blockers.
    // Recording it changes NOTHING (auto-unpause forbidden) — the task
    // stays PAUSED until the human himself runs resume.
    let ovr = devai(
        h,
        &[
            "task",
            "override",
            &b_id,
            "--rationale",
            "proceeding despite the cancelled backfill",
        ],
    );
    assert!(ovr.contains("override recorded"), "{ovr}");
    assert_eq!(
        devai_json(h, &["task", "get", &b_id])["state"],
        "PAUSED",
        "the override alone resumed nothing (OQ2)"
    );
    devai(h, &["task", "resume", &b_id]);
    assert_eq!(devai_json(h, &["task", "get", &b_id])["state"], "DRAFT");

    // The override act is stored history with the covered blockers.
    let evs = devai_json(h, &["task", "events", &b_id]);
    let over = evs
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "task.dependency.overridden")
        .expect("override event recorded");
    assert!(
        over["payload"]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == &a_id),
        "blocker covered: {over}"
    );

    // REQ-DEP-06 + OQ8: `b` CARRIES a parent edge (to `a`), so it is not a
    // tree root; `a` is parent-less with an incoming 'parent' edge — the
    // epic registry names exactly `a`.
    let epics = devai_json(h, &["task", "epics"]);
    let ids: Vec<&str> = epics
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["task"]["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&a_id.as_str()) && !ids.contains(&b_id.as_str()),
        "epic = parent-less vertex WITH children: {ids:?}"
    );
}

#[test]
fn acceptance_gate_reads_the_graph_and_write_time_integrity_is_loud_via_cli() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    let e_id = id_of(&devai_json(
        h,
        &[
            "task",
            "new",
            "Gated acceptance",
            "--description",
            "accept must consult the graph",
        ],
    ));
    let d_id = id_of(&devai_json(
        h,
        &[
            "task",
            "new",
            "The blocker",
            "--description",
            "open elsewhere",
        ],
    ));

    // REQ-DEP-07 AC1: retroactive registration from the CLI, either moment.
    let linked = devai(
        h,
        &["task", "depend", &e_id, "--on", &d_id, "--kind", "blocks"],
    );
    assert!(
        linked.contains("—blocks→"),
        "typed edge recorded retroactively:\n{linked}"
    );
    let j = devai_json(h, &["task", "dependencies", &e_id]);
    assert_eq!(j["outgoing"].as_array().unwrap().len(), 1);

    // REQ-DEP-07 AC2: integrity violations fail LOUD at the CLI, not silently.
    let err = devai_fails(
        h,
        &["task", "depend", &e_id, "--on", &e_id, "--kind", "parent"],
    );
    assert!(err.contains("itself"), "self-reference refused: {err}");
    let err = devai_fails(
        h,
        &["task", "depend", &e_id, "--on", &d_id, "--kind", "blocks"],
    );
    assert!(
        err.contains("already exists"),
        "duplicate edge refused: {err}"
    );
    let err = devai_fails(
        h,
        &[
            "task",
            "depend",
            &e_id,
            "--on",
            "deadbeef0000",
            "--kind",
            "parent",
        ],
    );
    assert!(
        err.contains("not found"),
        "dangling reference refused: {err}"
    );
    devai_fails(
        h,
        &[
            "task",
            "depend",
            &e_id,
            "--on",
            &d_id,
            "--kind",
            "friendship",
        ],
    );

    // Drive E to QA with everything the gates require: CURRENT spec, approval,
    // implementation report, quality report — so the ONLY thing stopping
    // acceptance is the open dependency (the refusal text proves it).
    devai(h, &["task", "start", &e_id]);
    let spec = write(h, "spec.json", "{\"requirements\": []}");
    devai(
        h,
        &[
            "artifact",
            "publish",
            &e_id,
            "--kind",
            "REQUIREMENTS_SPEC",
            "--file",
            &spec,
        ],
    );
    devai(h, &["task", "review", &e_id, "--verdict", "approved"]);
    devai(h, &["task", "approve", &e_id]);
    let rep = write(
        h,
        "impl.json",
        "{\"branch\":\"rework/x\",\"patch\":\"p\",\"test_results\":{}}",
    );
    devai(
        h,
        &[
            "artifact",
            "publish",
            &e_id,
            "--kind",
            "IMPLEMENTATION_REPORT",
            "--file",
            &rep,
        ],
    );
    let qr = write(h, "quality.json", "{\"verdict\":\"PASS\"}");
    devai(
        h,
        &[
            "artifact",
            "publish",
            &e_id,
            "--kind",
            "QUALITY_REPORT",
            "--file",
            &qr,
        ],
    );

    // REQ-DEP-03 at the ACCEPTANCE gate: refused, blocker enumerated as data.
    let err = devai_fails(h, &["task", "accept", &e_id]);
    assert!(
        err.contains("open blocking dependencies") && err.contains(&d_id),
        "acceptance names the blocker: {err}"
    );
    assert_eq!(
        devai_json(h, &["task", "get", &e_id])["state"],
        "QA",
        "the refused gate moved nothing"
    );

    // RATIFIED DECISION OQ1 at the human gate: CANCELLING the blocker does
    // NOT close the edge — a dead dependency is an unresolved problem, and
    // re-tuning the graph is the human's own act.
    devai(
        h,
        &[
            "task",
            "cancel",
            &d_id,
            "--reason",
            "cancelled, not resolved",
        ],
    );
    let err = devai_fails(h, &["task", "accept", &e_id]);
    assert!(
        err.contains("open blocking dependencies") && err.contains(&d_id),
        "CANCELLED does not close (OQ1): {err}"
    );

    // RATIFIED DECISION OQ7 at the CLI: removal = SUPERSEDE. The edge leaves
    // the live graph, its row stays as history — and the same acceptance
    // command now passes, while `dependencies` shows an empty live set.
    let un = devai(
        h,
        &["task", "unlink", &e_id, "--on", &d_id, "--kind", "blocks"],
    );
    assert!(un.contains("superseded"), "edge superseded:\n{un}");
    let j = devai_json(h, &["task", "dependencies", &e_id]);
    assert_eq!(
        j["outgoing"].as_array().unwrap().len(),
        0,
        "superseded edge is not live"
    );
    devai(h, &["task", "accept", &e_id]);
    assert_eq!(devai_json(h, &["task", "get", &e_id])["state"], "ACCEPTED");
    // The supersede act is stored history.
    let evs = devai_json(h, &["task", "events", &e_id]);
    assert!(
        evs.as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "task.dependency.unlinked"),
        "unlinked event recorded"
    );
}

/// REQ-DEP-07: the linking act is itself recorded as platform history through
/// the CLI, visible via `task events` with the same ids and kind.
#[test]
fn cli_depend_lands_in_the_event_history() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let a_id = id_of(&devai_json(h, &["task", "new", "A", "--description", ""]));
    let b = devai_json(
        h,
        &["task", "new", "B", "--description", "", "--parent", &a_id],
    );
    let b_id = id_of(&b);
    let evs = devai_json(h, &["task", "events", &b_id]);
    let linked = evs
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "task.dependency.linked")
        .expect("the creation-time link is in the history");
    assert_eq!(linked["payload"]["depends_on_id"], a_id.as_str());
    assert_eq!(linked["payload"]["dependency_kind"], "parent");
    assert_eq!(linked["actor"], "dep-human");
}
