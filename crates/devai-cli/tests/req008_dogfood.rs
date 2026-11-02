//! REQ-008: one full DOGFOODED cycle through the real `devai` binary —
//! exactly the surface a human drives — proving the QA freshness gate:
//!
//!   1. a fresh task runs analyst → reviewer → (human approve) →
//!      implementation → QA: every role execution is bound platform-side
//!      to the workspace revision, and the quality-report@4 embeds it;
//!   2. re-running on the UNCHANGED workspace must NOT silently re-run the
//!      auditor: the run stops at the freshness gate and asks for an
//!      explicit decision (zero new executions, one `qa.fresh_awaiting_decision`
//!      event);
//!   3. `task run --force-reaudit` honors the explicit human override,
//!      re-runs the audit, and persists the override as an attributable
//!      decision + `qa.reaudit_forced` event.
//!
//! The workspace is an isolated git+cargo repo in a tempdir: nothing else
//! on the machine can perturb its digest between the two runs, so the
//! freshness equality is proven, not assumed. Probes inside it are the
//! real factory `quality@3` profile (build/tests gates pass; devai-repo-
//! specific checks honestly FAIL — the verdict is irrelevant, the binding
//! and the gate behavior are what this dogfood certifies).

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("dogfood-human")
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

const CLEAN_TREE_DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[test]
fn dogfood_full_cycle_fresh_then_forced_reaudit() {
    let home = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    // -- an isolated, comparable workspace: committed + dirty untracked file
    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname = \"dogfood-ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "pub fn ready() -> bool {\n    true\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn ready_is_true() {\n        assert!(super::ready());\n    }\n}\n").unwrap();
    std::fs::write(ws.path().join(".gitignore"), "target/\n").unwrap();
    git(ws.path(), &["init", "-q"]);
    git(ws.path(), &["add", "-A"]);
    git(
        ws.path(),
        &["-c", "user.email=dogfood@test.local", "-c", "user.name=devai-dogfood", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"],
    );
    std::fs::write(ws.path().join("loose.txt"), b"untracked, non-ignored, part of the digest\n").unwrap();
    let head = git(ws.path(), &["rev-parse", "HEAD"]);

    // -- platform init: contracts @4 (and legacy @3) resolve, profile snapshot seeded
    devai(home.path(), &["init"]);
    let c4 = devai_json(home.path(), &["contract", "get", "quality-report", "--version", "4"]);
    assert!(c4.to_string().contains("workspace_revision"), "@4 machine gate demands the revision block");
    devai_json(home.path(), &["contract", "get", "quality-report", "--version", "3"]); // @1..@3 stay loadable (REQ-004)

    // -- create + drive to the human approval gate
    let created = devai_json(home.path(), &["task", "new", "Dogfood REQ-008", "--description", "Prove the QA freshness gate through the real CLI"]);
    let id = created["id"].as_str().unwrap().to_string();
    let ws_str = ws.path().to_str().unwrap();
    let r1 = devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r1["gate"], "WAITING_APPROVAL", "{r1}");
    devai(home.path(), &["task", "approve", &id, "--rationale", "dogfood approval"]);

    // -- full cycle into QA: platform-assembled quality-report@4 is published
    let r2 = devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r2["gate"], "WAITING_ACCEPT", "{r2}");

    // REQ-001/006/003: EVERY role execution — mock roles included — carries a
    // real, comparable revision of the same shape (no model involvement).
    let execs = devai_json(home.path(), &["exec", "list", "--task", &id]);
    let execs = execs.as_array().unwrap();
    assert!(execs.len() >= 3, "analyst + implementation + QA ran: {execs:?}");
    for e in execs {
        let rev = &e["workspace_revision"];
        assert!(
            rev["git_head"].as_str().map(|s| s == &head).unwrap_or(false),
            "{} execution bound to the real HEAD: {rev}",
            e["role_slug"]
        );
        let digest = rev["dirty_digest"].as_str().unwrap();
        assert_eq!(digest.len(), 64, "dirty digest is a SHA-256 hex");
        assert_ne!(digest, CLEAN_TREE_DIGEST, "untracked non-ignored file must move the digest (REQ-003)");
        assert_eq!(rev["profile"].as_str(), Some("quality@3"), "evaluation profile identity name@version");
        assert!(!rev["roles"].as_array().unwrap().is_empty(), "participating roles slug@version");
    }

    // REQ-004 + no-drift: the @4 report embeds the exact revision of its execution row.
    let rep = devai_json(home.path(), &["task", "report", &id]);
    assert_eq!(rep["seq"], 1);
    assert_eq!(rep["contract"], "quality-report@4");
    let rep_exec_id = rep["execution"].as_str().unwrap().to_string();
    let rep_exec = execs.iter().find(|e| e["id"].as_str() == Some(&rep_exec_id)).expect("report execution row exists");
    assert_eq!(rep_exec["workspace_revision"], rep["report"]["workspace_revision"], "report and execution row never drift");
    println!(
        "──── dogfood evidence: quality-report@4 embedded workspace_revision ────\n{}",
        serde_json::to_string_pretty(&rep["report"]["workspace_revision"]).unwrap()
    );

    // -- REQ-005: re-run on the UNCHANGED workspace asks, never re-audits.
    let events_before = devai_json(home.path(), &["task", "events", &id]).as_array().unwrap().len();
    let fresh_text = devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    println!("──── dogfood evidence: freshness re-run stdout ────\n{fresh_text}");
    assert!(
        fresh_text.contains("is FRESH — it is bound to exactly the recomputed workspace revision; the auditor was NOT re-run"),
        "human-facing freshness verdict must be explicit:\n{fresh_text}"
    );
    assert!(fresh_text.contains("--force-reaudit"), "the ask names the explicit override path:\n{fresh_text}");

    let execs_after = devai_json(home.path(), &["exec", "list", "--task", &id]).as_array().unwrap().clone();
    assert_eq!(execs_after.len(), execs.len(), "freshness gate started ZERO new executions");
    assert_eq!(devai_json(home.path(), &["task", "report", &id])["seq"], 1, "still the same report");

    let events = devai_json(home.path(), &["task", "events", &id]);
    let events = events.as_array().unwrap();
    assert_eq!(events.len(), events_before + 1, "the only trace is one event: the ask");
    let fresh_ev = events.iter().find(|e| e["kind"] == "qa.fresh_awaiting_decision").expect("fresh ask is an immutable event");
    assert_eq!(fresh_ev["payload"]["workspace_revision"], rep["report"]["workspace_revision"], "the gate compared against exactly the embedded revision");

    // -- REQ-005 override surface: explicit human decision re-audits, attributable.
    let forced_text = devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str, "--force-reaudit"]);
    let first_lines: String = forced_text.lines().filter(|l| l.starts_with('⏸') || l.contains("→") || l.starts_with("ВЕРДИКТ") || l.contains("НЕ ПРОВЕРЕНО")).take(6).collect::<Vec<_>>().join("\n");
    println!("──── dogfood evidence: --force-reaudit stdout (fragments) ────\n{first_lines}");
    assert!(forced_text.contains("quality report (platform-assembled)"), "the re-audit produced a fresh report rendering:\n{forced_text}");

    let execs_forced = devai_json(home.path(), &["exec", "list", "--task", &id]).as_array().unwrap().clone();
    assert!(execs_forced.len() > execs_after.len(), "--force-reaudit actually re-ran the audit");
    let rep2 = devai_json(home.path(), &["task", "report", &id]);
    assert_eq!(rep2["seq"], 2, "the re-audit published report v2 as CURRENT");
    assert_eq!(rep2["report"]["workspace_revision"], rep["report"]["workspace_revision"], "workspace is still unchanged — same binding");

    let events = devai_json(home.path(), &["task", "events", &id]);
    let events = events.as_array().unwrap();
    assert!(events.iter().any(|e| e["kind"] == "qa.reaudit_forced"), "the override is attributable in the event log");
    let forced = events.iter().rev().find(|e| e["kind"] == "qa.reaudit_forced").unwrap();
    assert_eq!(forced["actor"].as_str(), Some("dogfood-human"), "and attributed to the human who decided");
}
