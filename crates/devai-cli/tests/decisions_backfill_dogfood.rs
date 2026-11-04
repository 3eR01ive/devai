//! REQ-BF-007: the decisions backfill (rung 004) dogfooded through the REAL
//! `devai` binary. An isolated tempdir home carries seeded historical
//! `requirements.approved` / `requirements.rejected` events WITHOUT decision
//! rows (the pre-REQ-QAD-001 shape — seeded via PRAGMA user_version like the
//! store migration_tests do), and the next binary invocation opens the store,
//! which applies the rung. The assertions follow the ratified acceptance
//! reading (spec OQ1 = variant (б)): the extracted facts become visible to
//! the QA auditor through the APPROVED_DECISIONS section of the persisted
//! auditor prompt — the human `task progress` view gains NO decisions
//! section. On top of that:
//!
//!   1. the rung lands on `open()`, rows carry the DETERMINISTIC ids and
//!      source-event refs of the ratified OQ2/OQ6 mechanism, the
//!      NULL-rationale event is extracted as a NULL row (OQ4), the modern
//!      event-and-decision pair is NOT extracted secondarily, `task
//!      progress` renders no DECISIONS section, fact events exist per
//!      (task, kind) group, and a re-forced version re-applies as a NO-OP
//!      (REQ-BF-002/004/005/006);
//!   2. end-to-end with the offline mock agent: after the backfill, the
//!      RENDERED auditor prompt (auditor_prompt.txt, REQ-QAD-004 observation
//!      surface) contains the seeded legacy rationales and the explicit
//!      "(none stored)" mark for the NULL one — the auditor really sees
//!      what the extraction created (REQ-BF-006, ratified OQ1(б)/OQ4).

use std::path::Path;
use std::process::Command;

use devai_core::types::{event_kind, json_field};
use devai_store::backfill_decision_id;
use rusqlite::{Connection, params};
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
    let conn = Connection::open(home.join("devai.db")).unwrap();
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
    drop(conn);
    (e_appr, e_rej, e_null)
}

/// Seed the MODERN write-path shape (commands.rs `task approve`: event AND
/// decision side by side, refs = the spec id) directly, while the rung is
/// still pending — such an event must not be extracted secondarily.
fn seed_modern_pair(home: &Path, task_id: &str) -> (String, String) {
    let e_modern = uuid::Uuid::now_v7().to_string();
    let rationale = "modern approve: already a stored decision";
    let conn = Connection::open(home.join("devai.db")).unwrap();
    conn.execute(
        "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,'human:now',?4,'2026-10-06T00:00:00.000000Z')",
        params![e_modern, task_id, event_kind::REQUIREMENTS_APPROVED, serde_json::json!({"rationale": rationale}).to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO decisions(id,task_id,kind,verdict,actor,rationale,refs,created_at) VALUES(?1,?2,'requirements_approval','APPROVED','human:now',?3,'[\"spec-id\"]','2026-10-06T00:00:00.000000Z')",
        params![uuid::Uuid::now_v7().to_string(), task_id, rationale],
    )
    .unwrap();
    drop(conn);
    (e_modern, rationale.into())
}

/// Raw decision rows in stored chronology, read straight from the database:
/// (id, kind, verdict, actor, rationale, refs).
fn read_decisions(home: &Path, task_id: &str) -> Vec<(String, String, String, String, Option<String>, Vec<String>)> {
    let conn = Connection::open(home.join("devai.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT id,kind,verdict,actor,rationale,refs FROM decisions WHERE task_id=?1 ORDER BY created_at")
        .unwrap();
    let rows = stmt
        .query_map(params![task_id], |r| {
            let refs_text: String = r.get(5)?;
            let rationale: Option<String> = r.get(4)?;
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                rationale,
                serde_json::from_str::<Vec<String>>(&refs_text).unwrap(),
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn version_of(home: &Path) -> i64 {
    let conn = Connection::open(home.join("devai.db")).unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

#[test]
fn backfill_runs_on_open_lands_deterministic_rows_and_progress_stays_clean() {
    let home = TempDir::new().unwrap();
    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "Backfill dogfood", "--description", "historical verdicts must reach the decisions table"]);
    let id = created["id"].as_str().unwrap().to_string();
    let (e_appr, e_rej, e_null) = seed_legacy_events(home.path(), &id);
    let (e_modern, modern_rationale) = seed_modern_pair(home.path(), &id);

    // Any command opens the store — the rung lands here.
    devai(home.path(), &["task", "events", &id]);
    assert_eq!(version_of(home.path()), 5, "the ladder walked 4 → 5");

    // The human view stays exactly as it was (ratified OQ1 variant (б)):
    // acceptance is the auditor's APPROVED_DECISIONS, not a new progress
    // section — the absence below is the contract, pinned.
    let progress = devai(home.path(), &["task", "progress", &id]);
    assert!(!progress.contains("DECISIONS"), "`task progress` must NOT render a DECISIONS section (ratified OQ1: variant (б), scope creep rejected):\n{progress}");
    let pj = devai_json(home.path(), &["task", "progress", &id]);
    assert!(pj.get("decisions").is_none(), "the progress JSON must not grow a decisions key");

    // The rows themselves, in stored chronology: reject (Sep), legacy
    // approve (Oct 1), NULL approve (Oct 2), then the untouched modern row.
    let rows = read_decisions(home.path(), &id);
    assert_eq!(rows.len(), 4, "3 extracted (NULL included, ratified OQ4) + the pre-existing modern row: {rows:?}");
    for (i, event) in [(0, &e_rej), (1, &e_appr), (2, &e_null)] {
        assert_eq!(rows[i].0, backfill_decision_id(event).to_string(), "the decision id is DETERMINISTICALLY derived from the source event id (ratified OQ2)");
        assert_eq!(rows[i].5, vec![event.clone()], "refs cite the source event id (ratified OQ6)");
        assert_eq!(rows[i].3, "human:legacy", "actor copied from the event column");
    }
    assert_eq!(rows[2].4, None, "the NULL-rationale approve is a NULL row, not a skip (ratified OQ4)");
    assert_eq!(rows[0].1, "requirements_rejection", "reject kind per ratified OQ3");
    assert_eq!(rows[0].2, "REJECTED");
    assert_eq!(rows[1].1, "requirements_approval");
    assert_eq!(rows[1].2, "APPROVED");
    assert_eq!(rows[1].4.as_deref().unwrap(), "legacy approve: keep `task active`, do NOT ship `exec list --active`", "byte-identical, never paraphrased");
    assert!(rows[1].4.as_deref().unwrap().contains("task active"), "the acceptance string is stored");
    // The modern event was not extracted secondarily (REQ-BF-005 AC1).
    assert_eq!(rows.iter().filter(|r| r.4.as_deref() == Some(modern_rationale.as_str())).count(), 1, "the disclosed rationale stays single");
    assert!(rows.iter().all(|r| !r.5.contains(&e_modern)), "the modern event id appears in nobody's refs");

    // One fact event per (task, source kind) group (ratified OQ5).
    let evs = devai_json(home.path(), &["task", "events", &id]);
    let facts: Vec<&serde_json::Value> = evs.as_array().unwrap().iter().filter(|e| e["kind"] == event_kind::DECISIONS_BACKFILLED).collect();
    assert_eq!(facts.len(), 2, "one event per (task, source kind) group: {facts:?}");
    for f in &facts {
        assert_eq!(f["actor"], "migration:005_decisions_backfill");
        assert_eq!(f["payload"][json_field::BACKFILL], "005_decisions_backfill");
        assert_eq!(f["payload"][json_field::SCRIPT_PATH], "migrations/005_decisions_backfill.sql");
    }
    let appr_fact = facts.iter().find(|f| f["payload"][json_field::KIND] == event_kind::REQUIREMENTS_APPROVED).unwrap();
    assert_eq!(appr_fact["payload"][json_field::SOURCE_EVENTS].as_array().unwrap().len(), 2, "e-appr AND e-null were extracted");

    // Idempotence (REQ-BF-005, ratified OQ2): re-forcing the version makes
    // the deterministic ids recompute identically and ON CONFLICT DO
    // NOTHING affect no row — no duplicate decision, no fact event.
    {
        let conn = Connection::open(home.path().join("devai.db")).unwrap();
        conn.execute_batch("PRAGMA user_version=3;").unwrap();
    }
    devai(home.path(), &["task", "events", &id]);
    assert_eq!(read_decisions(home.path(), &id).len(), 4, "a re-applied rung creates no decision row");
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
    // Ratified OQ4: the NULL-rationale act is a STORED fact the auditor
    // sees marked as such, not a silently skipped event.
    assert!(section.contains("(none stored)"), "the NULL-rationale backfilled decision renders the explicit mark (ratified OQ4):\n{section}");
    assert_eq!(version_of(home.path()), 5);
    println!("──── dogfood evidence: APPROVED_DECISIONS with backfilled rows ────\n{section}");
}
