//! The ONE status field of the model-bundle snapshot (task 01a1b71f,
//! REQ-MS-06) on a real store home.
//!
//! The status is COMPUTED from two stored evidence inputs — the newest probe
//! of the active identity and the newest finished execution row carrying that
//! identity's stamp — with one priority rule (the fresher evidence wins; a
//! tie favours the probe). These tests pin every branch:
//! * a later failed run beats an earlier ok probe and the answer SAYS the
//!   probe is older;
//! * a newer ok probe beats the failed run;
//! * equal times favour the probe;
//! * a failed row WITHOUT a class yields no more than «причина не
//!   классифицирована»;
//! * a dead probe verdict is «не работает» with that verdict, while timeout
//!   is «не доказано»;
//! * RUNNING rows and pre-v11 (unstamped) rows never decide status;
//! * no evidence at all is «неизвестно», not «живая»;
//! * the refusal banner suggests only a bundle with the latest FRESH ok
//!   probe — and never applies anything itself.

use chrono::{Duration, Utc};
use devai_api::Api;
use devai_core::types::*;
use devai_store::{BundleSwitch, Store};
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
    probe: { phrase: ping, timeout_ms: 500, fresh_secs: 900 }
  - id: claude-live
    label: Claude Live
    agent: claude
    driver: claude
    model: claude-1
    reasoning: medium
    probe: { phrase: ping, timeout_ms: 500, fresh_secs: 900 }
"#;

struct Fixture {
    home: TempDir,
    task: Id,
}

impl Fixture {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join("config")).unwrap();
        std::fs::write(home.path().join("config/models.yaml"), MODELS).unwrap();
        let store = Store::open(home.path()).unwrap();
        let task = Task::new("status fixture", "status evidence", None, Utc::now());
        store.create_task(&task).unwrap();
        Fixture {
            home,
            task: task.id,
        }
    }

    fn store(&self) -> Store {
        Store::open(self.home.path()).unwrap()
    }

    fn api(&self) -> Api {
        Api::open(self.home.path(), "operator").unwrap()
    }

    /// Mark the `hermes-live` identity as the bundle in force through the
    /// SAME store door the switch uses.
    fn mark_active(&self) {
        self.store()
            .apply_bundle_switch(&BundleSwitch {
                agent: "hermes".into(),
                driver: "hermes".into(),
                llm_provider: "deepseek".into(),
                model: "live-1".into(),
                reasoning: "high".into(),
                actor: "operator".into(),
                payload: serde_json::json!({"outcome": "applied", "reason": "manual"}),
                now: Utc::now(),
            })
            .unwrap();
    }

    fn probe(&self, bundle: &str, verdict: &str, latency: u64, at: Timestamp) {
        let (driver, llm, model, reasoning) = match bundle {
            "hermes-live" => ("hermes", "deepseek", "live-1", "high"),
            "claude-live" => ("claude", "", "claude-1", "medium"),
            other => panic!("unknown fixture bundle {other}"),
        };
        self.store()
            .insert_event_with(
                new_id(),
                None,
                event_kind::MODEL_PROBE,
                "operator",
                serde_json::json!({
                    (json_field::BUNDLE_ID): bundle,
                    (json_field::BUNDLE): {
                        (json_field::DRIVER): driver,
                        (json_field::LLM_PROVIDER): llm,
                        (json_field::MODEL): model,
                        (json_field::REASONING): reasoning,
                    },
                    (json_field::VERDICT): verdict,
                    (json_field::LATENCY_MS): latency,
                    (json_field::EXIT_CODE): serde_json::Value::Null,
                    (json_field::OUTPUT_EXCERPT): "",
                }),
                at,
            )
            .unwrap();
    }

    /// A FINISHED execution row with the hermes identity stamp. `class` None
    /// exercises the honest «причина не классифицирована» branch.
    fn finish_run(&self, status: ExecutionStatus, class: Option<&str>, at: Timestamp) {
        let store = self.store();
        let e = RoleExecution {
            id: new_id(),
            task_id: self.task,
            role_slug: "quality-auditor".into(),
            role_version: 1,
            agent_name: Some("hermes".into()),
            status: ExecutionStatus::Running,
            inputs: vec![],
            summary: None,
            error: None,
            started_at: at - Duration::seconds(5),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: Some(ExecutionBundle::new("hermes", "deepseek", "live-1", "high")),
        };
        store.start_execution(&e).unwrap();
        store
            .finish_execution_classified(&e, status, None, Some("run failed"), class, at)
            .unwrap();
    }

    /// An execution row that is still RUNNING (never evidence for status).
    fn running_run(&self) {
        let e = RoleExecution {
            id: new_id(),
            task_id: self.task,
            role_slug: "quality-auditor".into(),
            role_version: 1,
            agent_name: Some("hermes".into()),
            status: ExecutionStatus::Running,
            inputs: vec![],
            summary: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: Some(ExecutionBundle::new("hermes", "deepseek", "live-1", "high")),
        };
        self.store().start_execution(&e).unwrap();
    }
}

#[test]
fn a_later_failed_run_beats_an_ok_probe_and_the_answer_says_the_probe_is_older() {
    let f = Fixture::new();
    f.mark_active();
    let t0 = Utc::now() - Duration::seconds(400);
    f.probe("hermes-live", "ok", 42, t0);
    f.finish_run(
        ExecutionStatus::Failed,
        Some(failure_class::PROVIDER),
        t0 + Duration::seconds(100),
    );

    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "not_working", "the later evidence wins");
    assert_eq!(snap.status.evidence.as_deref(), Some("execution"));
    assert_eq!(snap.status.reason.as_deref(), Some(failure_class::PROVIDER));
    assert!(snap.status.probe_older, "the OK probe must be marked older");
    assert!(snap.status.latency_ms.is_none());
    // R8 quality return: the winning evidence is a RUN, so the freshness word
    // must say «свежий прогон» — never «свежая проба».
    assert_eq!(snap.status.freshness, "fresh");
    assert_eq!(snap.status.freshness_text, "свежий прогон");
    // The banner is part of the SAME snapshot and applies nothing.
    let banner = snap.failures.expect("not_working carries the banner");
    assert_eq!(banner.code, failure_class::PROVIDER);
    assert_eq!(
        banner.suggestion, None,
        "no fresh successful probe = no offer"
    );
}

#[test]
fn a_newer_ok_probe_beats_the_failed_run_and_carries_the_latency() {
    let f = Fixture::new();
    f.mark_active();
    let t0 = Utc::now() - Duration::seconds(400);
    f.finish_run(ExecutionStatus::Failed, Some(failure_class::PROVIDER), t0);
    f.probe("hermes-live", "ok", 42, t0 + Duration::seconds(100));

    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "live");
    assert_eq!(snap.status.evidence.as_deref(), Some("probe"));
    assert_eq!(snap.status.latency_ms, Some(42));
    assert!(!snap.status.probe_older);
    assert!(snap.failures.is_none(), "a live bundle does not glow");
}

#[test]
fn equal_times_favour_the_probe() {
    let f = Fixture::new();
    f.mark_active();
    let at = Utc::now() - Duration::seconds(100);
    f.finish_run(ExecutionStatus::Failed, Some(failure_class::PROVIDER), at);
    f.probe("hermes-live", "ok", 7, at);

    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "live", "tie goes to the probe");
    assert_eq!(snap.status.evidence.as_deref(), Some("probe"));
}

#[test]
fn a_failed_row_without_a_class_is_never_given_an_invented_reason() {
    let f = Fixture::new();
    f.mark_active();
    f.finish_run(
        ExecutionStatus::Failed,
        None,
        Utc::now() - Duration::seconds(10),
    );

    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "not_working");
    assert_eq!(
        snap.status.reason.as_deref(),
        Some("причина не классифицирована"),
        "the platform names only what the row carries"
    );
    let banner = snap.failures.expect("the banner still glows");
    assert_eq!(banner.code, "причина не классифицирована");
}

#[test]
fn a_dead_probe_verdict_is_not_working_but_timeout_is_not_proven() {
    let f = Fixture::new();
    f.mark_active();
    f.probe(
        "hermes-live",
        "no_credits",
        0,
        Utc::now() - Duration::seconds(10),
    );
    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "not_working");
    assert_eq!(snap.status.reason.as_deref(), Some("no_credits"));
    assert!(snap.failures.is_some());
    assert_eq!(snap.failures.unwrap().code, "no_credits");

    // A fresh `timeout` verdict does not prove the bundle dead.
    f.probe("hermes-live", "timeout", 0, Utc::now());
    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "not_proven");
    assert!(snap.status.reason.is_none());
    assert!(snap.failures.is_none(), "«не доказано» does not glow");
}

#[test]
fn running_and_unstamped_rows_never_decide_the_status() {
    let f = Fixture::new();
    f.mark_active();
    f.running_run();

    // A RUNNING row is not evidence; a row without the v11 stamp cannot be
    // matched to the identity at all (it reads «связка не записана»).
    let store = f.store();
    // Its own task: the single-writer index allows one RUNNING row per task,
    // and this row is about the STATUS rule, not about that invariant.
    let other = Task::new("legacy stamped task", "old row", None, Utc::now());
    store.create_task(&other).unwrap();
    let legacy = RoleExecution {
        id: new_id(),
        task_id: other.id,
        role_slug: "quality-auditor".into(),
        role_version: 1,
        agent_name: Some("hermes".into()),
        status: ExecutionStatus::Running,
        inputs: vec![],
        summary: None,
        error: None,
        started_at: Utc::now() - Duration::seconds(60),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        bundle: None,
    };
    store.start_execution(&legacy).unwrap();
    store
        .finish_execution_classified(
            &legacy,
            ExecutionStatus::Failed,
            None,
            Some("old row"),
            Some(failure_class::PROVIDER),
            Utc::now(),
        )
        .unwrap();

    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(
        snap.status.state, "unknown",
        "no stamped evidence = «неизвестно», not «живая» and not «не работает»"
    );
    assert_eq!(snap.status.freshness, "unknown");
    assert_eq!(snap.status.freshness_text, "доказательств нет");
    assert!(!snap.running.is_empty(), "the RUNNING row is listed");
    assert_eq!(
        snap.running.len(),
        1,
        "only the RUNNING row: the finished legacy row is not in flight"
    );
    assert!(snap.status.evidence.is_none());
    assert!(snap.failures.is_none());
}

#[test]
fn a_switch_alone_leaves_the_status_unknown() {
    let f = Fixture::new();
    f.mark_active();
    let snap = f.api().model_snapshot().unwrap();
    let active = snap.active.expect("the switch marks a record");
    assert_eq!(active.id.as_deref(), Some("hermes-live"));
    assert_eq!(snap.status.state, "unknown", "a switch is not evidence");
    assert!(snap.status.latency_ms.is_none());
}

#[test]
fn the_banner_offers_only_a_bundle_with_the_latest_fresh_ok_probe() {
    let f = Fixture::new();
    f.mark_active();
    f.finish_run(
        ExecutionStatus::Failed,
        Some(failure_class::PROVIDER),
        Utc::now() - Duration::seconds(5),
    );
    // Live, but stale (older than the declared 900 s window): NOT an offer.
    f.probe(
        "claude-live",
        "ok",
        10,
        Utc::now() - Duration::seconds(2_000),
    );
    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(snap.status.state, "not_working");
    assert_eq!(
        snap.failures.unwrap().suggestion,
        None,
        "a stale success is not a live suggestion"
    );

    // The same bundle probed again inside the window becomes the offer.
    f.probe("claude-live", "ok", 11, Utc::now() - Duration::seconds(10));
    let snap = f.api().model_snapshot().unwrap();
    assert_eq!(
        snap.failures.unwrap().suggestion.as_deref(),
        Some("claude-live")
    );
}
