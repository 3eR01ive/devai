//! The model-bundle CONSOLE MIRROR on a real home (task 01a1b71f,
//! REQ-MS-01..08): `devai model list|probe|switch` against a temp store and
//! fake driver binaries on the child's PATH.
//!
//! What these pins prove:
//! * the allow-list is `models.yaml` and nothing else — an unknown id, a key
//!   in the file or a `mock` bundle is refused typed, never accepted;
//! * one probe is ONE stored event and changes no record or marker;
//! * a switch applies ONLY on `ok`, in one transaction: the agent record is
//!   upserted by name (its id survives), the single marker moves, the
//!   `model.switched` event carries actor/from/to/probe/reason;
//! * a dead probe refuses with the SAME verdict and leaves the record and
//!   the marker untouched;
//! * `no_op` and an unknown id refuse BEFORE any event, so the history count
//!   is an assertion, not a hope;
//! * switching to another agent NAME keeps both records, preserves the old
//!   record's identity and flags `agent_changed`;
//! * the snapshot names the driver / LLM-provider / model separately and
//!   carries no key, token or base_url.
//!
//! The drivers are shell scripts printed by the test itself, so no provider
//! call is made and the verdict classification still exercises the real
//! process path (`devai-runtime::probe`). `PATH` is set per child process,
//! so the tests stay parallel-safe.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use devai_store::Store;
use serde_json::Value;
use tempfile::TempDir;

const MODELS: &str = r#"
bundles:
  - id: hermes-live
    label: Hermes Live
    agent: hermes
    driver: hermes
    llm_provider: deepseek
    model: live-1
    reasoning: high
    probe: { phrase: ping, timeout_ms: 5000, fresh_secs: 900 }
  - id: hermes-dead
    label: Hermes Dead
    agent: hermes
    driver: hermes
    model: dead-1
    reasoning: high
    probe: { phrase: ping, timeout_ms: 5000, fresh_secs: 900 }
  - id: hermes-live-low
    label: Hermes Live · low
    agent: hermes
    driver: hermes
    llm_provider: deepseek
    model: live-1
    reasoning: low
    probe: { phrase: ping, timeout_ms: 5000, fresh_secs: 900 }
  - id: claude-live
    label: Claude Live
    agent: claude
    driver: claude
    model: claude-1
    reasoning: medium
    probe: { phrase: ping, timeout_ms: 5000, fresh_secs: 900 }
"#;

/// A home with the declared allow-list already in place.
fn home_with(models: &str) -> TempDir {
    let home = TempDir::new().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config/models.yaml"), models).unwrap();
    home
}

/// Fake drivers: `hermes` answers 402 for model `dead-1` and succeeds
/// otherwise; `claude` always succeeds. They accept (and ignore) the real
/// argv so the launch shape is the production one.
fn driver_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let hermes = dir.path().join("hermes");
    std::fs::write(
        &hermes,
        "#!/bin/sh\ncase \"$*\" in\n  *dead-1*) echo 'Error code: 402 - {\"error\":\"Insufficient Balance\"}' >&2; exit 1;;\nesac\necho pong\nexit 0\n",
    )
    .unwrap();
    let claude = dir.path().join("claude");
    std::fs::write(&claude, "#!/bin/sh\necho '{\"result\":\"pong\"}'\nexit 0\n").unwrap();
    for p in [hermes, claude] {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

fn path_with(drivers: &Path) -> String {
    let base = std::env::var("PATH").unwrap_or_default();
    format!("{}:{base}", drivers.display())
}

fn devai(home: &Path, drivers: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("operator")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .env_remove("DEVAI_EVIDENCE_DIR")
        .env("PATH", path_with(drivers))
        .args(args)
        .output()
        .expect("the devai binary is runnable")
}

fn devai_json(home: &Path, drivers: &Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend(args.iter().copied());
    let out = devai(home, drivers, &full);
    assert!(
        out.status.success(),
        "devai {args:?} --json failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print one JSON object ({e}): {text}"))
}

fn open(home: &Path) -> Store {
    Store::open(home).unwrap()
}

#[test]
fn a_clean_home_declares_bundles_and_selects_none() {
    let home = home_with(MODELS);
    let drivers = driver_dir();
    let snap = devai_json(home.path(), drivers.path(), &["model", "list"]);

    assert!(snap["active"].is_null(), "no marker = no bundle in force");
    assert_eq!(snap["status"]["state"], "not_selected");
    let ids: Vec<&str> = snap["declared"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["hermes-live", "hermes-dead", "hermes-live-low", "claude-live"]);
    // Four fields, never one «provider»: the declared view carries the
    // config vocabulary (REQ-MS-01 + REQ-MS-09).
    let first = &snap["declared"][0];
    assert_eq!(first["driver"], "hermes");
    assert_eq!(first["llm_provider"], "deepseek");
    assert_eq!(first["model"], "live-1");
    assert_eq!(first["reasoning"], "high");
    // The store holds exactly the seeded records and no marker.
    assert!(open(home.path()).active_agent().unwrap().is_none());
}

#[test]
fn one_probe_is_one_event_and_changes_nothing() {
    let home = home_with(MODELS);
    let drivers = driver_dir();

    let probe = devai_json(
        home.path(),
        drivers.path(),
        &["model", "probe", "hermes-live"],
    );
    assert_eq!(probe["verdict"], "ok");
    assert!(probe["latency_ms"].is_u64());

    let st = open(home.path());
    let events = st.model_events().unwrap();
    assert_eq!(events.len(), 1, "one call = exactly one event");
    assert_eq!(events[0].kind, "model.probe");
    assert!(events[0].task_id.is_none(), "a probe is not a task fact");
    let payload = &events[0].payload;
    assert_eq!(payload["bundle_id"], "hermes-live");
    assert_eq!(payload["verdict"], "ok");
    assert_eq!(payload["bundle"]["driver"], "hermes");
    assert_eq!(payload["bundle"]["llm_provider"], "deepseek");
    assert_eq!(payload["bundle"]["model"], "live-1");
    // No state change: no record, no marker.
    assert!(st.active_agent().unwrap().is_none());
}

#[test]
fn a_switch_applies_one_transaction_with_record_marker_and_history() {
    let home = home_with(MODELS);
    let drivers = driver_dir();

    let result = devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live", "--reason", "limits"],
    );
    assert_eq!(result["outcome"], "applied");
    assert_eq!(result["verdict"], "ok");

    let st = open(home.path());
    let active = st.active_agent().unwrap().expect("the switch marks a record");
    assert_eq!(active.name, "hermes");
    assert_eq!(active.provider, "hermes", "driver");
    assert_eq!(active.runtime, "deepseek", "LLM-provider");
    assert_eq!(active.model, "live-1");
    assert_eq!(active.reasoning, "high", "REQ-MS-09: the level is stored");
    assert!(active.active);
    // Exactly ONE marked row (the partial unique index plus the code path).
    assert_eq!(
        st.list_agents().unwrap().iter().filter(|a| a.active).count(),
        1
    );

    // Probe + switch, application order, one event each.
    let events = st.model_events().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, "model.probe");
    assert_eq!(events[1].kind, "model.switched");
    let switched = &events[1].payload;
    assert_eq!(events[1].actor, "operator");
    assert!(switched["from"].is_null(), "there was no bundle before");
    assert_eq!(switched["outcome"], "applied");
    assert_eq!(switched["reason"], "limits", "the human chose the reason");
    assert_eq!(switched["verdict"], "ok");
    assert_eq!(switched["bundle"]["model"], "live-1");
    assert_eq!(
        switched["bundle"]["reasoning"],
        "high",
        "the event carries the level as part of the `to` snapshot"
    );
    assert_eq!(switched["bundle_id"], "hermes-live");
    assert_eq!(switched["agent_changed"], false);
    assert_eq!(
        switched["probe_event"].as_str().unwrap(),
        events[0].id.to_string(),
        "the switch cites the probe it rested on"
    );

    // The snapshot shows the new bundle, ONE status word and the latency.
    let snap = devai_json(home.path(), drivers.path(), &["model", "list"]);
    assert_eq!(snap["active"]["agent"], "hermes");
    assert_eq!(snap["active"]["driver"], "hermes");
    assert_eq!(snap["active"]["llm_provider"], "deepseek");
    assert_eq!(snap["active"]["model"], "live-1");
    assert_eq!(snap["active"]["reasoning"], "high");
    assert_eq!(snap["active"]["id"], "hermes-live");
    assert_eq!(snap["status"]["state"], "live");
    assert!(snap["status"]["latency_ms"].is_u64());
    assert_eq!(snap["status"]["freshness"], "fresh");
    assert_eq!(snap["history"].as_array().unwrap().len(), 2);
    assert_eq!(snap["history"][1]["outcome"], "applied");
    assert_eq!(snap["history"][1]["reason"], "limits");

    // No secret material anywhere in the surface (REQ-MS-08).
    let text = serde_json::to_string(&snap).unwrap();
    for forbidden in ["api_key", "base_url", "token", "secret"] {
        assert!(
            !text.contains(forbidden),
            "the snapshot must not carry `{forbidden}`: {text}"
        );
    }
}

#[test]
fn a_dead_probe_refuses_with_its_verdict_and_leaves_the_bundle_alone() {
    let home = home_with(MODELS);
    let drivers = driver_dir();
    devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live"],
    );

    let refused = devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-dead", "--reason", "limits"],
    );
    assert_eq!(refused["outcome"], "refused_probe");
    assert_eq!(refused["verdict"], "no_credits");

    let st = open(home.path());
    let active = st.active_agent().unwrap().unwrap();
    assert_eq!(active.model, "live-1", "the record did not move");
    assert!(active.active, "the marker did not move");

    let events = st.model_events().unwrap();
    assert_eq!(events.len(), 4, "probe+switch twice — one event each");
    assert_eq!(events[2].kind, "model.probe");
    assert_eq!(events[2].payload["verdict"], "no_credits");
    assert_eq!(events[3].kind, "model.switched");
    assert_eq!(events[3].payload["outcome"], "refused_probe");
    assert_eq!(events[3].payload["verdict"], "no_credits");
    assert_eq!(events[3].payload["reason"], "limits");

    // The surface keeps showing the LIVE bundle, not the refused target.
    let snap = devai_json(home.path(), drivers.path(), &["model", "list"]);
    assert_eq!(snap["active"]["id"], "hermes-live");
    assert_eq!(snap["status"]["state"], "live");
}

#[test]
fn no_op_unknown_id_and_a_key_in_the_file_are_refused_without_events() {
    let home = home_with(MODELS);
    let drivers = driver_dir();
    devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live"],
    );
    let before = open(home.path()).model_events().unwrap().len();

    let same = devai(home.path(), drivers.path(), &["model", "switch", "hermes-live"]);
    assert!(!same.status.success(), "a no_op is a refusal");
    assert!(
        String::from_utf8_lossy(&same.stderr).contains("no_op"),
        "the refusal is typed: {}",
        String::from_utf8_lossy(&same.stderr)
    );

    let unknown = devai(home.path(), drivers.path(), &["model", "switch", "nope"]);
    assert!(!unknown.status.success());
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("unknown_bundle"), "{stderr}");
    assert!(
        stderr.contains("hermes-live") && stderr.contains("claude-live"),
        "the refusal lists the declared ids: {stderr}"
    );
    assert_eq!(
        open(home.path()).model_events().unwrap().len(),
        before,
        "precondition refusals write nothing"
    );

    // A key/base_url in the file is refused by NAME at load (REQ-MS-02/08).
    let bad = home_with("bundles:\n  - id: a\n    label: A\n    agent: a\n    driver: hermes\n    base_url: https://x\n");
    let out = devai(bad.path(), drivers.path(), &["model", "list"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("base_url"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // `mock` is never a bundle.
    let mock = home_with("bundles:\n  - id: m\n    label: M\n    agent: m\n    driver: mock\n    reasoning: high\n");
    let out = devai(mock.path(), drivers.path(), &["model", "list"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("mock"));
}

#[test]
fn switching_the_agent_name_keeps_both_records_and_flags_the_change() {
    let home = home_with(MODELS);
    let drivers = driver_dir();
    devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live"],
    );
    let hermes_id = open(home.path()).active_agent().unwrap().unwrap().id;

    let result = devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "claude-live"],
    );
    assert_eq!(result["outcome"], "applied");

    let st = open(home.path());
    let active = st.active_agent().unwrap().unwrap();
    assert_eq!(active.name, "claude");
    assert_eq!(active.provider, "claude");
    assert_eq!(active.model, "claude-1");
    // The previous record still exists, keeps ITS identity, and is unmarked.
    let hermes = st
        .list_agents()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "hermes")
        .expect("the previous agent record is preserved");
    assert_eq!(hermes.id, hermes_id, "the record id is stable");
    assert_eq!(hermes.model, "live-1");
    assert!(!hermes.active);
    assert_eq!(st.list_agents().unwrap().iter().filter(|a| a.active).count(), 1);

    let events = st.model_events().unwrap();
    let switched = &events.last().unwrap().payload;
    assert_eq!(switched["agent_changed"], true);
    assert_eq!(switched["from"]["driver"], "hermes");
    assert_eq!(switched["from"]["model"], "live-1");
    assert_eq!(switched["bundle"]["driver"], "claude");

    // Back to the same agent NAME: the upsert keeps the original id.
    devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live"],
    );
    let back = open(home.path()).active_agent().unwrap().unwrap();
    assert_eq!(back.id, hermes_id, "upsert by name preserves the id");
    assert_eq!(back.model, "live-1");
}

/// REQ-MS-09 / the ratified OQ-6 answer: the level is PART of the bundle, so
/// switching between two bundles that share agent+driver+provider+model but
/// differ ONLY by the level is a real switch, not a `no_op`. This is the
/// whole point of the answer: the operator changes the level in one step.
#[test]
fn switching_only_the_reasoning_level_applies_and_is_not_a_no_op() {
    let home = home_with(MODELS);
    let drivers = driver_dir();
    devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live"],
    );
    let before = open(home.path()).model_events().unwrap().len();

    let result = devai_json(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live-low", "--reason", "manual"],
    );
    assert_eq!(result["outcome"], "applied");
    let active = open(home.path()).active_agent().unwrap().unwrap();
    assert_eq!(active.model, "live-1", "same model");
    assert_eq!(active.reasoning, "low", "the level changed in place");

    // Probe+switch, exactly one event each — the no_op path was NOT taken.
    let events = open(home.path()).model_events().unwrap();
    assert_eq!(events.len(), before + 2);
    let switched = &events.last().unwrap().payload;
    assert_eq!(switched["from"]["reasoning"], "high");
    assert_eq!(switched["bundle"]["reasoning"], "low");

    // And back: the same model + the same level is now a genuine no_op.
    let same = devai(
        home.path(),
        drivers.path(),
        &["model", "switch", "hermes-live-low"],
    );
    assert!(!same.status.success());
    assert!(
        String::from_utf8_lossy(&same.stderr).contains("no_op"),
        "{}",
        String::from_utf8_lossy(&same.stderr)
    );
}

/// REQ-MS-09: a bundle without a level never loads — the load names the
/// field, and no surface can select an incomplete bundle.
#[test]
fn a_bundle_without_a_reasoning_level_fails_the_load() {
    let home = home_with(
        "bundles:\n  - id: no-level\n    label: No Level\n    agent: hermes\n    driver: hermes\n    model: m\n",
    );
    let drivers = driver_dir();
    let out = devai(home.path(), drivers.path(), &["model", "list"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("reasoning"), "{stderr}");
}

/// REQ-MS-02 AC1: a CLEAN home gets the declared-bundle file from `init`,
/// and an existing copy is never overwritten — the operator owns the list.
#[test]
fn init_materialises_models_yaml_and_never_overwrites_an_existing_copy() {
    let home = TempDir::new().unwrap();
    let drivers = driver_dir();
    let out = devai(home.path(), drivers.path(), &["init"]);
    assert!(
        out.status.success(),
        "devai init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = home.path().join("config/models.yaml");
    assert!(path.exists(), "a clean home receives models.yaml");

    let custom = "bundles:\n  - id: mine\n    label: Mine\n    agent: hermes\n    driver: hermes\n    model: live-1\n";
    std::fs::write(&path, custom).unwrap();
    let out = devai(home.path(), drivers.path(), &["init"]);
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        custom,
        "an existing copy is preserved byte-for-byte"
    );
}
