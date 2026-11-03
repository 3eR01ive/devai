//! REQ-QAD-004: the QA auditor must actually SEE the human's recorded
//! decisions — driven through the real `devai` binary with the offline mock
//! agent (a dogfood cycle, not a unit mockup):
//!
//!   1. a task runs analyst → reviewer to the approval gate;
//!   2. the human approves with a rationale containing the string `OQ1=X`
//!      (`task approve --rationale` persists the act as a STORED decision
//!      row — the fact the auditor is fed, REQ-QAD-001);
//!   3. implementation → QA; the platform persists the RENDERED auditor
//!      prompt as a run artifact (`auditor_prompt.txt` next to `probes.txt`,
//!      the observation surface that resolves OQ-F);
//!   4. the test asserts on that actual prompt: the rationale appears
//!      INSIDE the APPROVED_DECISIONS section, and the section sits exactly
//!      between the implementation report and the mechanical evidence
//!      (fixed order, REQ-QAD-002).
//!   5. a `--force-reaudit` re-run on the unchanged workspace rebuilds the
//!      prompt from the same stored facts: the section-label sequence is
//!      identical (determinism), and the override decision itself is now
//!      visible to the second audit.
//!
//! The workspace is an isolated git+cargo repo in a tempdir, as in
//! req008_dogfood.rs; probe verdicts are irrelevant — what is certified
//! here is the auditor's INPUT.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("qad-human")
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

/// The persisted auditor prompt of the execution that published the CURRENT
/// quality report (the report names its execution; evidence lives under
/// `<home>/evidence/<task>/<execution>/` — platform-owned, REQ-WT-05).
fn auditor_prompt(home: &Path, task_id: &str) -> String {
    let rep = devai_json(home, &["task", "report", task_id]);
    let exec = rep["execution"].as_str().expect("the CURRENT report names its execution").to_string();
    let path = home.join("evidence").join(task_id).join(&exec).join("auditor_prompt.txt");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("the QA stage must persist the rendered auditor prompt at {}: {e}", path.display()))
}

/// The section-label line sequence — the prompt's observable structure.
fn section_labels(prompt: &str) -> Vec<String> {
    prompt.lines().filter(|l| l.starts_with("--- ")).map(|l| l.to_string()).collect()
}

#[test]
fn dogfood_qa_auditor_prompt_carries_approved_decision_rationale() {
    let home = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname = \"qad-dogfood-ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "pub fn ready() -> bool { true }\n").unwrap();
    std::fs::write(ws.path().join(".gitignore"), "target/\n").unwrap();
    git(ws.path(), &["init", "-q"]);
    git(ws.path(), &["add", "-A"]);
    git(ws.path(), &["-c", "user.email=qad@test.local", "-c", "user.name=devai-qad", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"]);
    let ws_str = ws.path().to_str().unwrap();

    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "QAD dogfood", "--description", "The auditor must be fed the approved decisions of the task"]);
    let id = created["id"].as_str().unwrap().to_string();

    // -- analyst + reviewer: stop at the human approval gate.
    let r1 = devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r1["gate"], "WAITING_APPROVAL", "{r1}");

    // -- THE human act under test: approve WITH a rationale that ratifies
    //    an open question. This must become a stored decision row.
    devai(home.path(), &["task", "approve", &id, "--rationale", "OQ1=X: use `task active`, NOT `exec list --active` — the rejected alternative must not ship"]);

    // -- implementation + QA in one driven cycle; stops at acceptance.
    let r2 = devai_json(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r2["gate"], "WAITING_ACCEPT", "{r2}");

    let prompt = auditor_prompt(home.path(), &id);

    // 1) the APPROVED_DECISIONS section exists, and the human's string is
    //    inside it — not somewhere else in the prompt.
    let dec_start = prompt.find("--- APPROVED_DECISIONS").expect("the QA prompt must carry an APPROVED_DECISIONS section");
    let mech_start = prompt.find("--- MECHANICAL_EVIDENCE ").expect("the probe section follows the decisions");
    assert!(dec_start < mech_start, "decisions must precede the mechanical evidence");
    let section = &prompt[dec_start..mech_start];
    assert!(section.contains("OQ1=X"), "the ratified open-question resolution must be in the decisions section, got:\n{section}");
    //    stored fields reproduced: kind, verdict, actor.
    assert!(section.contains("kind: requirements_approval"), "the approve decision row must be reproduced:\n{section}");
    assert!(section.contains("verdict: APPROVED"), "{section}");
    assert!(section.contains("actor: qad-human"), "{section}");

    // 2) fixed section order: spec → impl report → decisions → evidence.
    let i_spec = prompt.find("--- REQUIREMENTS_SPEC v").expect("spec section");
    let i_impl = prompt.find("--- IMPLEMENTATION_REPORT v").expect("implementation report section");
    assert!(i_spec < i_impl && i_impl < dec_start, "section order spec → impl-report → decisions is broken");

    // 3) determinism (REQ-QAD-002 AC2): a force-reaudit on the UNCHANGED
    //    workspace rebuilds the prompt from the same stored facts — the
    //    label sequence must repeat exactly (paths inside one section may
    //    differ per execution id; the STRUCTURE is what must not drift).
    let labels_before = section_labels(&prompt);
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str, "--force-reaudit"]);
    let prompt2 = auditor_prompt(home.path(), &id);
    assert_eq!(section_labels(&prompt2), labels_before, "two builds over the same stored facts must yield the identical section sequence");
    let dec_start2 = prompt2.find("--- APPROVED_DECISIONS").unwrap();
    let mech_start2 = prompt2.find("--- MECHANICAL_EVIDENCE ").unwrap();
    let section2 = &prompt2[dec_start2..mech_start2];
    assert!(section2.contains("OQ1=X"), "the re-audit still sees the approve rationale");
    //    the override itself is a stored decision (engine.rs) and the newest
    //    row in the section (created_at order, OQ-D).
    assert!(section2.contains("qa_reaudit_override"), "decisions recorded during the run are stored facts the next assembly reads:\n{section2}");

    println!("──── dogfood evidence: APPROVED_DECISIONS section (audit 2) ────\n{section2}");
}
