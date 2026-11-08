//! The SWITCH surface on the PROBE SEAM (task 01a1b71f, REQ-MS-03/04/05,
//! R8 quality return). An injected runner returns a chosen verdict, so the
//! apply/refuse paths are exercised without launching a provider process.
//!
//! What these pins prove:
//! * a dead verdict refuses with the SAME verdict, moves nothing, writes one
//!   `refused_probe` fact and NEVER claims an agent-name change (the refusal
//!   is not a switch);
//! * an `ok` verdict applies the record+marker+event and the probe's words
//!   come from the ONE core dictionary (`verdict_text`/`reason_text`).

use std::sync::Arc;

use chrono::Utc;
use devai_api::{Api, ProbeRunner};
use devai_core::types::*;
use devai_runtime::probe::{ProbeOutcome, ProbeRequest};
use devai_store::Store;
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
  - id: claude-live
    label: Claude Live
    agent: claude
    driver: claude
    model: claude-1
    reasoning: medium
"#;

struct Fixture {
    home: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join("config")).unwrap();
        std::fs::write(home.path().join("config/models.yaml"), MODELS).unwrap();
        Store::open(home.path()).unwrap();
        Fixture { home }
    }

    /// An Api whose probe seam always answers with `verdict` and `latency`.
    fn api(&self, verdict: ProbeVerdict, latency: u64) -> Api {
        let runner: ProbeRunner = Arc::new(move |_r: &ProbeRequest| ProbeOutcome {
            verdict,
            latency_ms: latency,
            exit_code: Some(if verdict == ProbeVerdict::Ok { 0 } else { 1 }),
            output_excerpt: "seam evidence".into(),
            at: Utc::now(),
        });
        Api::open(self.home.path(), "operator")
            .unwrap()
            .with_probe_runner(runner)
    }

    fn events(&self) -> Vec<WorkflowEvent> {
        Store::open(self.home.path())
            .unwrap()
            .model_events()
            .unwrap()
    }
}

#[test]
fn an_ok_result_applies_and_the_words_come_from_one_dictionary() {
    let f = Fixture::new();
    let api = f.api(ProbeVerdict::Ok, 33);
    let probe = api.model_probe("hermes-live").unwrap();
    assert_eq!(probe.verdict, "ok");
    assert_eq!(probe.verdict_text, "успешна");

    let applied = api.model_switch("hermes-live", "manual").unwrap();
    assert_eq!(applied.outcome, "applied");
    // No marker existed before: there is no NAME to have changed (ASM-9).
    assert!(!applied.agent_changed);

    let snap = api.model_snapshot().unwrap();
    assert_eq!(snap.active.unwrap().id.as_deref(), Some("hermes-live"));
    assert_eq!(snap.status.state, "live");
    assert_eq!(snap.status.state_text, "живая");
    assert_eq!(snap.status.freshness_text, "свежая проба");
    // The same normalized words the console prints — one dictionary.
    assert_eq!(failure_word("no_credits"), "нет кредитов");
}

#[test]
fn a_dead_verdict_refuses_and_never_claims_an_agent_change() {
    let f = Fixture::new();
    // First select the live bundle.
    f.api(ProbeVerdict::Ok, 10)
        .model_switch("hermes-live", "manual")
        .unwrap();

    // Then aim at another AGENT NAME whose probe proves dead: the refusal
    // must NOT report a name change (nothing moved).
    let refused = f
        .api(ProbeVerdict::NoCredits, 0)
        .model_switch("claude-live", "limits")
        .unwrap();
    assert_eq!(refused.outcome, "refused_probe");
    assert_eq!(refused.verdict, "no_credits");
    assert!(!refused.agent_changed, "a refusal never changes the agent");

    let events = f.events();
    let last = events.last().unwrap();
    assert_eq!(last.kind, event_kind::MODEL_SWITCHED);
    assert_eq!(last.payload[json_field::OUTCOME], "refused_probe");
    assert_eq!(last.payload[json_field::VERDICT], "no_credits");
    assert_eq!(
        last.payload[json_field::AGENT_CHANGED],
        false,
        "the history must not claim a change the refusal did not make"
    );
    assert_eq!(last.payload[json_field::REASON], "limits");
    assert!(last.payload.get(json_field::FAILURE_STEP).is_none());

    // The record in force is still the first bundle.
    let snap = f.api(ProbeVerdict::Ok, 1).model_snapshot().unwrap();
    assert_eq!(snap.active.unwrap().id.as_deref(), Some("hermes-live"));
    assert_eq!(snap.status.reason, None);
    assert_eq!(
        snap.history.last().unwrap().reason_text.as_deref(),
        Some("limits")
    );
}

#[test]
fn a_dead_probe_verdict_is_worded_from_the_dictionary() {
    let f = Fixture::new();
    let probe = f
        .api(ProbeVerdict::NoKey, 0)
        .model_probe("hermes-live")
        .unwrap();
    assert_eq!(probe.verdict, "no_key");
    assert_eq!(probe.verdict_text, "нет ключа провайдера");
}
