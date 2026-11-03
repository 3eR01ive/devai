//! REQ-BF-007: the decisions backfill (rung 004) dogfooded through the REAL
//! `devai` binary. An isolated tempdir home carries seeded historical
//! `requirements.approved` / `requirements.rejected` events WITHOUT decision
//! rows (the pre-REQ-QAD-001 shape — seeded via PRAGMA user_version like the
//! store migration_tests do), and the next binary invocation opens the store,
//! which applies the rung. The test then asserts the extracted facts are
//! VISIBLE exactly where the acceptance names them:
//!
//!   1. the DECISIONS section of `devai task progress` shows the stored
//!      rationale byte-for-byte (REQ-BF-006 acceptance reading (a));
//!   2. `decisions.backfilled` fact events exist with the migration actor,
//!      and the null-rationale event was NOT extracted (REQ-BF-002/004);
//!   3. a re-forced version re-applies the rung as a NO-OP (REQ-BF-005);
//!   4. end-to-end with the offline mock agent: after the backfill, the
//!      RENDERED auditor prompt (auditor_prompt.txt, REQ-QAD-004 observation
//!      surface) contains the seeded legacy rationale — the auditor really
//!      sees what the extraction created (REQ-BF-006).

use std::path::Path;
use std::process::Command;

use devai_core::types::{event_kind, json_field};
use rusqlite::params;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("bf-human")
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Seed pre-v4 history: the version is pushed back to 3 and legacy verdict
/// events are written directly (the shape the old write-path left behind —
/// rationale only in the payload, no decision rows).
fn seed_legacy_events(home: &Path, task_id: &str) -> (String, String, String) {
    let e_appr = uuid::Uuid::now_v7().to_string();
    let e_rej = uuid::Uuid::now_v7().to_string();
    let e_null = uuid::Uuid::now_v7().to_string();
    let conn = rusqlite::Connection::open(home.join("devai.db")).unwrap();
    conn.execute_batch("PRAGMA user_version=3;").unwrap();
    for (id, kind, payload, ts) in [
        (&e_rej, event_kind::REQUIREMENTS_REJECTED, serde_json::json!({"rationale": "legacy reject: split OQ1 first"}), "2026-09-01T00:00:00.000000Z"),
        (&e_appr, event_kind::REQUIREMENTS_APPROVED, serde_json::json!({"requirements_seq": 1, "rationale": "legacy approve: keep `task active`, do NOT ship `exec list --active`"}), "2026-10-01T00:00:00.000000Z"),
        (&e_null, event_kind::REQUIREMENTS_APPROVED, serde_json::json!({"requirements_seq": 2, "rationale": null}), "2026-10-02T00:00:00.000000Z"),
    ] {
        conn.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,'human:legacy',?4,?5)",
            params![id, task_id, kind, payload.to_string(), ts],
        )
        .unwrap();
    }
    (e_appr, e_rej, e_null)
}

fn version_of(home: &Path) -> i64 {
    let conn = rusqlite::Connection::open(home.join("devai.db")).unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

#[test]
fn backfill_runs_on_open_and_shows_the_ratified_rationale_in_task_progress() {
    let home = TempDir::new().unwrap();
    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "Backfill dogfood", "--description", "historical verdicts must reach the decisions table"]);
    let id = created["id"].as_str().unwrap().to_string();
    let (e_appr, e_rej, e_null) = seed_legacy_events(home.path(), &id);

    // Any command opens the store — the rung lands here.
    let progress = devai(home.path(), &["task", "progress", &id]);
    assert_eq!(version_of(home.path()), 4, "the ladder walked 3 → 4");

    // 1) DECISIONS section: the stored rationale, reproduced verbatim.
    assert!(progress.contains("DECISIONS:"), "the progress view must carry a DECISIONS section:\n{progress}");
    assert!(progress.contains("legacy approve: keep `task active`, do NOT ship `exec list --active`"), "the acceptance string 'task active' must be visible:\n{progress}");
    assert!(progress.contains("requirements_rejection") && progress.contains("REJECTED"), "the reject event got its own row:\n{progress}");

    let pj = devai_json(home.path(), &["task", "progress", &id]);
    let decisions = pj["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2, "approve + reject extracted, the NULL-rationale approve skipped: {decisions:?}");
    let appr = decisions.iter().find(|d| d["kind"] == "requirements_approval").unwrap();
    assert_eq!(appr["verdict"], "APPROVED");
    assert_eq!(appr["actor"], "human:legacy", "actor copied from the event column");
    assert_eq!(appr["refs"].as_array().unwrap()[0], e_appr.as_str(), "the row cites its source event");
    assert_eq!(appr["rationale"].as_str().unwrap(), "legacy approve: keep `task active`, do NOT ship `exec list --active`", "byte-identical, never paraphrased");
    assert!(decisions.iter().all(|d| d["refs"][0] != e_null.as_str()), "the null-rationale event created nothing");
    assert!(decisions.iter().all(|d| d["refs"][0] != e_rej.as_str() || d["kind"] == "requirements_rejection"));

    // 2) one fact event per group, migration actor, citing both sides.
    let evs = devai_json(home.path(), &["task", "events", &id]);
    let facts: Vec<&serde_json::Value> = evs.as_array().unwrap().iter().filter(|e| e["kind"] == event_kind::DECISIONS_BACKFILLED).collect();
    assert_eq!(facts.len(), 2, "one event per (task, source kind) group");
    for f in &facts {
        assert_eq!(f["actor"], "migration:004_decisions_backfill");
        assert_eq!(f["payload"][json_field::BACKFILL], "004_decisions_backfill");
        assert_eq!(f["payload"][json_field::SCRIPT_PATH], "migrations/004_decisions_backfill.sql");
    }
    let appr_fact = facts.iter().find(|f| f["payload"][json_field::KIND] == event_kind::REQUIREMENTS_APPROVED).unwrap();
    assert_eq!(appr_fact["payload"][json_field::SOURCE_EVENTS].as_array().unwrap().len(), 1, "only e-appr was extracted");

    // 3) idempotence: re-forcing the version and re-opening is a no-op.
    {
        let conn = rusqlite::Connection::open(home.path().join("devai.db")).unwrap();
        conn.execute_batch("PRAGMA user_version=3;").unwrap();
    }
    let pj2 = devai_json(home.path(), &["task", "progress", &id]);
    assert_eq!(pj2["decisions"].as_array().unwrap().len(), 2, "a re-applied rung creates no decision row");
    let evs2 = devai_json(home.path(), &["task", "events", &id]);
    assert_eq!(evs2.as_array().unwrap().iter().filter(|e| e["kind"] == event_kind::DECISIONS_BACKFILLED).count(), 2, "a re-applied rung creates no fact event");
}

#[test]
fn backfilled_legacy_rationale_reaches_the_auditor_prompt() {
    let home = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname = \"bf-dogfood-ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "pub fn ready() -> bool { true }\n").unwrap();
    std::fs::write(ws.path().join(".gitignore"), "target/\n").unwrap();
    git(ws.path(), &["init", "-q"]);
    git(ws.path(), &["add", "-A"]);
    git(ws.path(), &["-c", "user.email=bf@test.local", "-c", "user.name=devai-bf", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"]);

    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "Backfill auditor", "--description", "the auditor must see historical rationales too"]);
    let id = created["id"].as_str().unwrap().to_string();
    let ws_str = ws.path().to_str().unwrap();

    let r1 = devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r1["gate"], "WAITING_APPROVAL", "{r1}");

    // Seed legacy history AT the approval gate and let the next command
    // (the migration) land before the modern approve follows.
    seed_legacy_events(home.path(), &id);
    devai(home.path(), &["task", "events", &id]);

    devai(home.path(), &["task", "approve", &id, "--rationale", "modern approve after backfill"]);
    let r2 = devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r2["gate"], "WAITING_ACCEPT", "{r2}");

    let rep = devai_json(home.path(), &["task", "report", &id]);
    let exec = rep["execution"].as_str().expect("CURRENT report names its execution").to_string();
    let prompt_path = home.path().join("evidence").join(&id).join(&exec).join("auditor_prompt.txt");
    let prompt = std::fs::read_to_string(&prompt_path).unwrap_or_else(|e| panic!("auditor prompt must be persisted at {}: {e}", prompt_path.display()));
    let dec_start = prompt.find("--- APPROVED_DECISIONS").expect("APPROVED_DECISIONS section");
    let mech_start = prompt.find("--- MECHANICAL_EVIDENCE ").expect("evidence section");
    let section = &prompt[dec_start..mech_start];
    // The backfilled legacy rows stand in the auditor's input next to the
    // live ones — the spec-fidelity scorer can no longer miss `task active`.
    assert!(section.contains("legacy approve: keep `task active`"), "the seeded historical rationale must reach the auditor:\n{section}");
    assert!(section.contains("legacy reject: split OQ1 first"), "reject rationales are visible too:\n{section}");
    assert!(section.contains("modern approve after backfill"), "live rows are unaffected:\n{section}");
    assert_eq!(version_of(home.path()), 4);
    println!("──── dogfood evidence: APPROVED_DECISIONS with backfilled rows ────\n{section}");
}
