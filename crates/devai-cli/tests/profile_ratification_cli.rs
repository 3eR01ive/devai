//! The HUMAN command surface of project-rules ratification (task 01a1bacf,
//! RV1/RV9/RV10) through the real `devai` binary: `profile status` on a
//! fresh home, `profile ratify` by the operator, the act in `task
//! decisions`, an edit invalidating force, and `profile revoke`.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    run(home, args, false, true)
}

/// ONE runner for every CLI call: `role_session` decides the session kind the
/// door declares, because the CLI classifies its own process from the
/// platform-exported run marker (`DEVAI_EVIDENCE_DIR`, RV4 AC2) — a test that
/// leaked the marker into a "human" call would silently exercise the role
/// door instead of the operator's.
fn run(home: &Path, args: &[&str], role_session: bool, expect_success: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_devai"));
    cmd.arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("operator")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .env_remove("DEVAI_EVIDENCE_DIR")
        .args(args);
    if role_session {
        cmd.env("DEVAI_EVIDENCE_DIR", home.join("evidence-run"));
    }
    let out = cmd.output().expect("the devai binary is runnable");
    if expect_success {
        assert!(
            out.status.success(),
            "devai {args:?} exited {:?}\nstdout: {}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        assert!(
            !out.status.success(),
            "devai {args:?} must fail, got stdout: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}

fn devai_json(home: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json"];
    full.extend(args.iter().copied());
    let s = devai(home, &full);
    serde_json::from_str(s.trim())
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print JSON ({e}): {s}"))
}

fn devai_fail(home: &Path, args: &[&str]) -> String {
    run(home, args, false, false)
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
            "profile",
            "ratify",
            "--task",
            &tid,
            "--project",
            "alpha",
            "--items",
            "dom/one",
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
    assert_eq!(
        out["decision"]["kind"],
        serde_json::json!("profile_ratification")
    );
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
    assert_eq!(
        rev["decision"]["payload"]["revokes"],
        serde_json::json!(decision)
    );
    assert_eq!(fs::read(&rules).unwrap(), before);
    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(false));
    assert_eq!(
        state["items"][0]["reason"],
        serde_json::json!("ратификация отозвана")
    );
}

/// RV4 AC2 through the REAL binary: the platform exports `DEVAI_EVIDENCE_DIR`
/// to every role process it spawns, so a `devai profile ratify` started
/// inside a role run is refused by the API's session check — the actor string
/// (here the operator's own) buys nothing. The same call from a shell without
/// the marker succeeds.
#[test]
fn a_role_run_cannot_use_the_human_door() {
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
    let ratify_args = [
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
    ];

    let err = run(home.path(), &ratify_args, true, false);
    assert!(err.contains("HUMAN-ONLY"), "{err}");
    // Nothing landed: the carrier task has no decision and the item is OUT.
    let decisions = devai_json(home.path(), &["task", "decisions", &tid]);
    assert_eq!(decisions.as_array().unwrap().len(), 0, "{decisions}");
    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(false));

    // The human shell (no run marker) performs the act.
    devai(home.path(), &ratify_args);
    let state = devai_json(home.path(), &["profile", "status", "alpha"]);
    assert_eq!(state["items"][0]["in_force"], serde_json::json!(true));
}

/// ux/contract-stability: the HUMAN plane of the new command is pinned too.
/// The printed lines are what an operator reads (the JSON is checked above),
/// so a wording drift of the status/ratify/revoke views fails HERE.
#[test]
fn the_human_printed_plane_is_pinned() {
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

    // Fresh home: the view states the edition and announces the item as OUT.
    let status = devai(home.path(), &["profile", "status", "alpha"]);
    assert!(status.starts_with("project: alpha ("), "{status}");
    assert!(
        status.contains(&format!("rules:   profiles/{pid}.overlay.yaml")),
        "{status}"
    );
    let edition = status
        .lines()
        .find_map(|l| l.strip_prefix("edition: "))
        .expect("the status view prints the edition");
    assert!(edition.starts_with("sha256:"), "{status}");
    assert!(status.contains("items:   1 (in force: 0)"), "{status}");
    assert!(
        status.contains("  OUT       dom/one  нератифицирован"),
        "{status}"
    );

    // The act: the operator sees the decision, the edition and the actor.
    let out = devai(
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
    let decision = out
        .lines()
        .find_map(|l| l.strip_prefix("ratified: decision "))
        .and_then(|rest| rest.split(' ').next())
        .expect("the ratify view names the decision");
    assert!(out.contains("(recorded)"), "{out}");
    assert!(
        out.contains(&format!(
            "project: {pid}  file: profiles/{pid}.overlay.yaml"
        )),
        "{out}"
    );
    assert!(out.contains(&format!("edition: {edition}")), "{out}");
    assert!(out.contains("items:   dom/one"), "{out}");
    assert!(
        out.contains("actor:   operator  rationale: правила проекта проверены"),
        "{out}"
    );

    // In force after the act.
    let status = devai(home.path(), &["profile", "status", "alpha"]);
    assert!(
        status.contains(&format!(
            "  IN FORCE  dom/one  by operator  decision {decision}"
        )),
        "{status}"
    );
    assert!(status.contains("items:   1 (in force: 1)"), "{status}");

    // The revocation view says what left force and that the file was kept.
    let rev = devai(
        home.path(),
        &[
            "profile",
            "revoke",
            decision,
            "--rationale",
            "правило отозвано",
        ],
    );
    assert!(rev.starts_with(&format!("revoked: decision ")), "{rev}");
    assert!(
        rev.contains(&format!("takes {decision} out of force")),
        "{rev}"
    );
    assert!(
        rev.contains("items:   dom/one — now OUT of force; the rules file was not touched"),
        "{rev}"
    );
    let status = devai(home.path(), &["profile", "status", "alpha"]);
    assert!(
        status.contains("  OUT       dom/one  ратификация отозвана"),
        "{status}"
    );
}

/// ux/contract-stability: the new command's help is snapshot-pinned like the
/// older subcommands (`docs/cli-snapshot/help_*.txt`). Refresh deliberately
/// with `UPDATE_PROFILE_HELP=1 cargo test -p devai-cli --test profile_ratification_cli`.
#[test]
fn profile_help_snapshot_has_no_drift() {
    let home = TempDir::new().unwrap();
    let help = devai(home.path(), &["profile", "--help"]);
    let snapshot =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli-snapshot/help_profile.txt");
    if std::env::var_os("UPDATE_PROFILE_HELP").is_some() {
        fs::write(&snapshot, &help).expect("write the profile help snapshot");
        return;
    }
    let stored = fs::read_to_string(&snapshot)
        .unwrap_or_else(|e| panic!("read {}: {e}", snapshot.display()));
    assert_eq!(
        stored.trim_end(),
        help.trim_end(),
        "help_profile snapshot drifted from `devai profile --help`"
    );
    for verb in ["status", "ratify", "revoke"] {
        assert!(
            stored.contains(verb),
            "the help must list `{verb}`: {stored}"
        );
    }
}
