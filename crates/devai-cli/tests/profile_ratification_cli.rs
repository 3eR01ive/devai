//! The HUMAN command surface of project-rules ratification (task 01a1bacf,
//! RV1/RV9/RV10) through the real `devai` binary: `profile status` on a
//! fresh home, `profile ratify` by the operator, the act in `task
//! decisions`, an edit invalidating force, and `profile revoke`.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("operator")
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
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print JSON ({e}): {s}"))
}

fn devai_fail(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("operator")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(
        !out.status.success(),
        "devai {args:?} must fail, got stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn overlay_body(pid: &str) -> String {
    format!(
        r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - kind: add
    target_id: dom/one
    ratified_by: ""
    add:
      statement: проектное правило
      detects: []
      verify: rubric
      anchors: {{"1.0": a, "0.5": b, "0.0": c}}
      weight: 0.2
"#
    )
}

#[test]
fn human_ratification_command_walks_the_three_states() {
    let home = TempDir::new().unwrap();
    devai(home.path(), &["init"]);
    devai(home.path(), &["project", "add", "alpha"]);
    let project = devai_json(home.path(), &["project", "get", "alpha"]);
    let pid = project["id"].as_str().unwrap().to_string();
    let rules = home
        .path()
        .join("config")
        .join("profiles")
        .join(format!("{pid}.overlay.yaml"));
    fs::create_dir_all(rules.parent().unwrap()).unwrap();
    fs::write(&rules, overlay_body(&pid)).unwrap();
    let task = devai_json(
        home.path(),
        &["task", "new", "носитель ратификации", "--project", "alpha"],
    );
    let tid = task["id"].as_str().unwrap().to_string();

    // 1) A home WITHOUT ratifications: the state view succeeds, names the
    // item as out of force and prints the edition fingerprint (RV9).
    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    let edition = state["edition"].as_str().unwrap().to_string();
    assert!(edition.starts_with("sha256:"));
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(false));
    assert_eq!(state["items"][0]["target_id"], serde_json::json!("dom/one"));

    // 2) The HUMAN act. A missing reason refuses and names the argument.
    let err = devai_fail(
        home.path(),
        &[
            "profile", "ratify", "--task", &tid, "--project", "alpha", "--items", "dom/one",
        ],
    );
    assert!(err.contains("--rationale"), "{err}");
    let out = devai_json(
        home.path(),
        &[
            "profile",
            "ratify",
            "--task",
            &tid,
            "--project",
            "alpha",
            "--items",
            "dom/one",
            "--rationale",
            "правила проекта проверены",
        ],
    );
    assert_eq!(out["recorded_now"], serde_json::json!(true));
    assert_eq!(out["decision"]["kind"], serde_json::json!("profile_ratification"));
    assert_eq!(out["decision"]["verdict"], serde_json::json!("APPROVED"));
    assert_eq!(out["decision"]["actor"], serde_json::json!("operator"));
    assert_eq!(out["edition"], serde_json::json!(edition));
    let decision = out["decision"]["id"].as_str().unwrap().to_string();

    // The act is visible in `task decisions` with the covered items.
    let decisions = devai_json(home.path(), &["task", "decisions", &tid]);
    let row = decisions
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["kind"] == serde_json::json!("profile_ratification"))
        .expect("the decision is listed");
    assert_eq!(row["verdict"], serde_json::json!("APPROVED"));
    assert_eq!(row["refs"], serde_json::json!(["dom/one"]));
    assert_eq!(row["payload"]["edition"], serde_json::json!(edition));

    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(true));
    assert_eq!(state["items"][0]["decision"], serde_json::json!(decision));

    // 3) Editing the rules content: force is gone (new ratification needed).
    let ratified = fs::read_to_string(&rules).unwrap();
    fs::write(&rules, ratified.replace("weight: 0.2", "weight: 0.3")).unwrap();
    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(false));
    assert_eq!(
        state["items"][0]["reason"],
        serde_json::json!("редакция изменена после ратификации")
    );

    // Restore the ratified edition: force returns without a new decision.
    fs::write(&rules, &ratified).unwrap();

    // 4) Revocation: a separate human act, the file is not rewritten.
    let before = fs::read(&rules).unwrap();
    let rev = devai_json(
        home.path(),
        &[
            "profile",
            "revoke",
            &decision,
            "--rationale",
            "правило отозвано",
        ],
    );
    assert_eq!(rev["decision"]["verdict"], serde_json::json!("REJECTED"));
    assert_eq!(rev["decision"]["payload"]["revokes"], serde_json::json!(decision));
    assert_eq!(fs::read(&rules).unwrap(), before);
    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(false));
    assert_eq!(
        state["items"][0]["reason"],
        serde_json::json!("ратификация отозвана")
    );
}
