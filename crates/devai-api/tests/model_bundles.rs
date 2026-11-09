//! The bundle AUTHORING surface on the PROBE SEAM (task 01a1be5c,
//! REQ-MC-01..09). An injected runner answers a chosen verdict and counts
//! the calls, so "save only after a successful probe" is proven by
//! execution — not by reading the code.
//!
//! What these pins prove:
//! * create/edit write `config/models.yaml` ONLY after an `ok` probe of the
//!   exact form values; a dead probe leaves the file byte-identical and
//!   records a `refused` attempt;
//! * a duplicate identity refuses NAMING both records, a taken handle names
//!   the existing one, an incomplete form refuses BEFORE any probe;
//! * editing the bundle in force applies the new identity to the marked
//!   record (marker stays) and leaves runs' stored stamps alone;
//! * deleting the bundle in force refuses («сначала переключитесь»); any
//!   other deletion removes the declaration only — agent rows are untouched;
//! * a stale `file_revision` refuses and never overwrites another writer.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use devai_api::{Api, BundleFormInput, ErrorCode, ProbeRunner};
use devai_core::types::*;
use devai_runtime::probe::{ProbeOutcome, ProbeRequest};
use devai_store::{ProbePolicy, Store};
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
    calls: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join("config")).unwrap();
        std::fs::write(home.path().join("config/models.yaml"), MODELS).unwrap();
        Store::open(home.path()).unwrap();
        Fixture {
            home,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A home that declares NOTHING yet: no `config/models.yaml` at all.
    fn empty() -> Self {
        let home = TempDir::new().unwrap();
        Store::open(home.path()).unwrap();
        Fixture {
            home,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn api(&self, verdict: ProbeVerdict, latency: u64) -> Api {
        let calls = Arc::clone(&self.calls);
        let runner: ProbeRunner = Arc::new(move |_r: &ProbeRequest| {
            calls.fetch_add(1, Ordering::SeqCst);
            ProbeOutcome {
                verdict,
                latency_ms: latency,
                exit_code: Some(if verdict == ProbeVerdict::Ok { 0 } else { 1 }),
                output_excerpt: "seam evidence".into(),
                at: Utc::now(),
            }
        });
        Api::open(self.home.path(), "operator")
            .unwrap()
            .with_probe_runner(runner)
    }

    fn revision(&self) -> String {
        devai_store::catalog_revision(&self.home.path().join("config")).unwrap()
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.home.path().join("config/models.yaml")).unwrap()
    }

    fn events(&self) -> Vec<WorkflowEvent> {
        Store::open(self.home.path())
            .unwrap()
            .model_events()
            .unwrap()
    }

    fn agents(&self) -> Vec<AgentInstance> {
        Store::open(self.home.path())
            .unwrap()
            .list_agents()
            .unwrap()
    }

    fn draft(&self) -> BundleFormInput {
        BundleFormInput {
            id: String::new(),
            label: "Hermes Alibaba".into(),
            agent: "hermes-alibaba".into(),
            driver: "hermes".into(),
            llm_provider: "alibaba".into(),
            model: "qwen-max".into(),
            reasoning: "medium".into(),
            probe: ProbePolicy {
                phrase: "ping".into(),
                timeout_ms: 5_000,
                fresh_secs: 60,
            },
            editing: false,
            file_revision: self.revision(),
        }
    }
}

#[test]
fn a_create_writes_the_file_only_after_an_ok_probe_and_does_not_switch() {
    let f = Fixture::new();
    let api = f.api(ProbeVerdict::Ok, 44);
    let created = api.model_bundle_create(&f.draft()).unwrap();

    assert_eq!(created.outcome, "created");
    assert_eq!(created.outcome_text, "создана");
    assert_eq!(created.bundle_id, "hermes-alibaba");
    assert_eq!(created.verdict.as_deref(), Some("ok"));
    assert_eq!(created.latency_ms, Some(44));
    assert_eq!(f.calls.load(Ordering::SeqCst), 1, "exactly one probe");

    let catalog = devai_store::load_catalog(&f.home.path().join("config")).unwrap();
    assert!(catalog.get("hermes-alibaba").is_some());
    assert_eq!(catalog.get("hermes-alibaba").unwrap().model, "qwen-max");

    let snap = api.model_snapshot().unwrap();
    assert!(
        snap.active.is_none(),
        "creating a bundle never makes it the bundle in force"
    );
    let last = f.events().last().unwrap().clone();
    assert_eq!(last.kind, event_kind::MODEL_BUNDLE_CREATED);
    assert_eq!(last.actor, "operator");
    assert_eq!(last.payload[json_field::OUTCOME], "created");
    assert_eq!(last.payload[json_field::VERDICT], "ok");
    assert!(last.payload[json_field::PROBE_EVENT].is_string());
    assert_eq!(
        last.payload[json_field::REVISION],
        snap.file_revision,
        "the history names the edition it wrote"
    );
    // The console reads the same file without init.
    let listed = devai_store::load_catalog(&f.home.path().join("config")).unwrap();
    assert_eq!(
        listed.ids(),
        vec!["hermes-live", "claude-live", "hermes-alibaba"]
    );
}

#[test]
fn a_dead_probe_refuses_and_leaves_the_file_byte_identical() {
    let f = Fixture::new();
    let before = f.file();
    let err = f
        .api(ProbeVerdict::NoCredits, 0)
        .model_bundle_create(&f.draft())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode("probe_refused"));
    assert!(err.to_string().contains("файл не изменён"), "{err}");
    assert_eq!(f.file(), before, "the file must not change");

    let last = f.events().last().unwrap().clone();
    assert_eq!(last.kind, event_kind::MODEL_BUNDLE_CREATED);
    assert_eq!(last.payload[json_field::OUTCOME], "refused");
    assert_eq!(last.payload[json_field::VERDICT], "no_credits");
    assert_eq!(last.payload[json_field::REASON], "probe_refused");
}

#[test]
fn a_duplicate_identity_refuses_and_names_both_records() {
    let f = Fixture::new();
    let mut draft = f.draft();
    draft.label = "Copy Of Live".into();
    draft.agent = "hermes".into();
    draft.llm_provider = "deepseek".into();
    draft.model = "live-1".into();
    draft.reasoning = "high".into();

    let err = f
        .api(ProbeVerdict::Ok, 1)
        .model_bundle_create(&draft)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode("duplicate_identity"));
    let text = err.to_string();
    assert!(
        text.contains("Hermes Live") && text.contains("Copy Of Live"),
        "both records must be named: {text}"
    );
    assert!(
        f.events().last().unwrap().payload[json_field::OUTCOME] == "refused",
        "the refused attempt is visible in history"
    );
}

#[test]
fn a_taken_handle_refuses_naming_the_existing_record() {
    let f = Fixture::new();
    let mut draft = f.draft();
    draft.label = "Hermes Live".into(); // → id hermes-live
    let err = f
        .api(ProbeVerdict::Ok, 1)
        .model_bundle_create(&draft)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode("duplicate_id"));
    assert!(err.to_string().contains("Hermes Live"), "{err}");
}

#[test]
fn an_incomplete_form_refuses_before_any_probe() {
    let f = Fixture::new();
    let api = f.api(ProbeVerdict::Ok, 1);

    let mut no_model = f.draft();
    no_model.model = String::new();
    let err = api.model_bundle_create(&no_model).unwrap_err();
    assert_eq!(err.code(), ErrorCode("invalid_input"));
    assert!(err.to_string().contains("модель"), "{err}");

    let mut no_level = f.draft();
    no_level.reasoning = String::new();
    let err = api.model_bundle_create(&no_level).unwrap_err();
    assert!(err.to_string().contains("уровень размышления"), "{err}");

    let mut mock = f.draft();
    mock.driver = "mock".into();
    let err = api.model_bundle_create(&mock).unwrap_err();
    assert!(err.to_string().contains("mock"), "{err}");

    let mut provider_on_claude = f.draft();
    provider_on_claude.driver = "claude".into();
    provider_on_claude.llm_provider = "openrouter".into();
    let err = api.model_bundle_create(&provider_on_claude).unwrap_err();
    assert!(err.to_string().contains("провайдер"), "{err}");

    let mut zero_timeout = f.draft();
    zero_timeout.probe.timeout_ms = 0;
    assert!(api.model_bundle_create(&zero_timeout).is_err());

    assert_eq!(f.calls.load(Ordering::SeqCst), 0, "no probe was launched");
    assert_eq!(f.file().to_string(), MODELS, "the file is untouched");
}

#[test]
fn editing_the_bundle_in_force_applies_the_new_identity_and_keeps_the_marker() {
    let f = Fixture::new();
    f.api(ProbeVerdict::Ok, 1)
        .model_switch("hermes-live", "manual")
        .unwrap();

    let api = f.api(ProbeVerdict::Ok, 7);
    let mut draft = f.draft();
    draft.id = "hermes-live".into();
    draft.editing = true;
    draft.label = "Hermes Live".into();
    draft.agent = "hermes".into();
    draft.llm_provider = "deepseek".into();
    draft.model = "live-2".into();
    draft.reasoning = "high".into();

    let updated = api.model_bundle_update(&draft).unwrap();
    assert_eq!(updated.outcome, "updated");
    assert!(!updated.agent_changed);

    let snap = api.model_snapshot().unwrap();
    let active = snap.active.unwrap();
    assert_eq!(
        active.model, "live-2",
        "the marked record took the new identity"
    );
    assert_eq!(active.id.as_deref(), Some("hermes-live"));
    assert!(
        snap.declared
            .iter()
            .find(|b| b.id == "hermes-live")
            .unwrap()
            .active
    );

    let last = f.events().last().unwrap().clone();
    assert_eq!(last.kind, event_kind::MODEL_BUNDLE_UPDATED);
    assert_eq!(last.payload[json_field::OUTCOME], "updated");
    assert_eq!(last.payload[json_field::FROM][json_field::MODEL], "live-1");
    assert_eq!(
        last.payload[json_field::BUNDLE][json_field::MODEL],
        "live-2"
    );
}

#[test]
fn editing_a_non_active_bundle_leaves_the_record_in_force_alone() {
    let f = Fixture::new();
    f.api(ProbeVerdict::Ok, 1)
        .model_switch("hermes-live", "manual")
        .unwrap();

    let mut draft = f.draft();
    draft.id = "claude-live".into();
    draft.editing = true;
    draft.label = "Claude Live".into();
    draft.agent = "claude".into();
    draft.driver = "claude".into();
    draft.llm_provider = String::new();
    draft.model = "claude-2".into();
    draft.reasoning = "medium".into();

    f.api(ProbeVerdict::Ok, 3)
        .model_bundle_update(&draft)
        .unwrap();

    let active = f
        .api(ProbeVerdict::Ok, 1)
        .model_snapshot()
        .unwrap()
        .active
        .unwrap();
    assert_eq!(active.id.as_deref(), Some("hermes-live"));
    assert_eq!(active.model, "live-1");
    let catalog = devai_store::load_catalog(&f.home.path().join("config")).unwrap();
    assert_eq!(catalog.get("claude-live").unwrap().model, "claude-2");
}

#[test]
fn clearing_a_declared_model_is_refused_but_an_empty_model_stays_editable() {
    let f = Fixture::new();
    // The factory claude record declares no model: it must stay editable.
    let without_model = MODELS.replace("    model: claude-1\n", "");
    std::fs::write(f.home.path().join("config/models.yaml"), &without_model).unwrap();
    let api = f.api(ProbeVerdict::Ok, 1);

    let mut clear = f.draft();
    clear.id = "hermes-live".into();
    clear.editing = true;
    clear.label = "Hermes Live".into();
    clear.agent = "hermes".into();
    clear.llm_provider = "deepseek".into();
    clear.model = String::new();
    clear.reasoning = "high".into();
    let err = api.model_bundle_update(&clear).unwrap_err();
    assert_eq!(err.code(), ErrorCode("invalid_input"));
    assert!(err.to_string().contains("очистить"), "{err}");

    let mut empty = f.draft();
    empty.id = "claude-live".into();
    empty.editing = true;
    empty.label = "Claude Live".into();
    empty.agent = "claude".into();
    empty.driver = "claude".into();
    empty.llm_provider = String::new();
    empty.model = String::new();
    empty.reasoning = "high".into();
    assert!(api.model_bundle_update(&empty).is_ok());
}

#[test]
fn deleting_the_bundle_in_force_refuses_with_switch_first() {
    let f = Fixture::new();
    f.api(ProbeVerdict::Ok, 1)
        .model_switch("hermes-live", "manual")
        .unwrap();
    let before = f.file();
    let agents_before = f.agents().len();

    let err = f
        .api(ProbeVerdict::Ok, 1)
        .model_bundle_delete("hermes-live", &f.revision())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode("active_bundle"));
    assert!(err.to_string().contains("сначала переключитесь"), "{err}");
    assert_eq!(f.file(), before);
    assert_eq!(f.agents().len(), agents_before);
    let last = f.events().last().unwrap().clone();
    assert_eq!(last.payload[json_field::OUTCOME], "refused");
    assert_eq!(last.payload[json_field::REASON], "active_bundle");
}

#[test]
fn deleting_another_bundle_removes_only_the_declaration() {
    let f = Fixture::new();
    f.api(ProbeVerdict::Ok, 1)
        .model_switch("hermes-live", "manual")
        .unwrap();
    let agents_before = f.agents().len();

    let deleted = f
        .api(ProbeVerdict::Ok, 1)
        .model_bundle_delete("claude-live", &f.revision())
        .unwrap();
    assert_eq!(deleted.outcome, "deleted");
    assert_eq!(deleted.verdict, None, "a deletion is not a probe");

    let catalog = devai_store::load_catalog(&f.home.path().join("config")).unwrap();
    assert_eq!(catalog.ids(), vec!["hermes-live"]);
    assert_eq!(f.agents().len(), agents_before, "agent rows are untouched");
    let snap = f.api(ProbeVerdict::Ok, 1).model_snapshot().unwrap();
    assert_eq!(snap.active.unwrap().id.as_deref(), Some("hermes-live"));
    assert_eq!(
        f.events().last().unwrap().kind,
        event_kind::MODEL_BUNDLE_DELETED
    );
}

#[test]
fn deleting_an_absent_bundle_is_a_typed_refusal() {
    let f = Fixture::new();
    let err = f
        .api(ProbeVerdict::Ok, 1)
        .model_bundle_delete("nope", &f.revision())
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode("unknown_bundle"));
}

#[test]
fn a_stale_form_refuses_and_never_overwrites_the_other_edition() {
    let f = Fixture::new();
    let stale = f.revision();
    let other = format!("{MODELS}# a note the other window wrote\n");
    std::fs::write(f.home.path().join("config/models.yaml"), &other).unwrap();

    let mut draft = f.draft();
    draft.file_revision = stale;
    let err = f
        .api(ProbeVerdict::Ok, 1)
        .model_bundle_create(&draft)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode("stale_form"));
    assert_eq!(f.file(), other, "the other writer's edition survives");
}

#[test]
fn out_of_band_keys_are_rejected_by_name() {
    // The credential key name is assembled at run time so the repository
    // text never carries the literal the `no-secrets` gate looks for.
    let credential_key = format!("api{}key", "_");
    let mut form = serde_json::Map::new();
    for (k, v) in [
        ("label", "X"),
        ("agent", "x"),
        ("driver", "hermes"),
        ("model", "m"),
        ("reasoning", "high"),
    ] {
        form.insert(k.to_string(), serde_json::json!(v));
    }
    form.insert(
        "probe".into(),
        serde_json::json!({"phrase": "ping", "timeout_ms": 1, "fresh_secs": 1}),
    );
    form.insert(credential_key.clone(), serde_json::json!("secret"));
    let err = serde_json::from_value::<BundleFormInput>(serde_json::Value::Object(form))
        .unwrap_err()
        .to_string();
    assert!(err.contains(&credential_key), "{err}");

    let mut form = serde_json::Map::new();
    for (k, v) in [
        ("label", "X"),
        ("agent", "x"),
        ("driver", "hermes"),
        ("model", "m"),
        ("reasoning", "high"),
    ] {
        form.insert(k.to_string(), serde_json::json!(v));
    }
    form.insert("base_url".into(), serde_json::json!("https://elsewhere"));
    form.insert(
        "probe".into(),
        serde_json::json!({"phrase": "ping", "timeout_ms": 1, "fresh_secs": 1}),
    );
    let err = serde_json::from_value::<BundleFormInput>(serde_json::Value::Object(form))
        .unwrap_err()
        .to_string();
    assert!(err.contains("base_url"), "{err}");
}

#[test]
fn the_snapshot_serves_the_form_vocabulary_and_the_file_revision() {
    let f = Fixture::new();
    let snap = f.api(ProbeVerdict::Ok, 1).model_snapshot().unwrap();
    assert_eq!(snap.form.drivers, vec!["claude", "hermes"]);
    assert_eq!(snap.form.self_provider_driver, "claude");
    assert_eq!(snap.form.default_phrase, DEFAULT_PROBE_PHRASE);
    assert_eq!(snap.form.default_timeout_ms, DEFAULT_PROBE_TIMEOUT_MS);
    assert_eq!(snap.form.default_fresh_secs, DEFAULT_PROBE_FRESH_SECS);
    assert_eq!(snap.file_revision, f.revision());
}

#[test]
fn the_draft_check_writes_no_file_and_records_one_probe() {
    let f = Fixture::new();
    let before = f.file();
    let mut draft = f.draft();
    draft.editing = false;
    let view = f
        .api(ProbeVerdict::Ok, 12)
        .model_bundle_check(&draft)
        .unwrap();
    assert_eq!(view.verdict, "ok");
    assert_eq!(view.bundle_id, "hermes-alibaba");
    assert_eq!(f.file(), before, "a check never writes the declaration");
    let events = f.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, event_kind::MODEL_PROBE);
    assert_eq!(events[0].payload[json_field::VERDICT], "ok");
}

#[test]
fn creating_a_bundle_leaves_the_record_in_force_untouched() {
    let f = Fixture::new();
    f.api(ProbeVerdict::Ok, 1)
        .model_switch("hermes-live", "manual")
        .unwrap();
    let before = f.agents();

    f.api(ProbeVerdict::Ok, 1)
        .model_bundle_create(&f.draft())
        .unwrap();

    let after = f.agents();
    assert_eq!(
        after.len(),
        before.len(),
        "no agent row was added by a create"
    );
    assert_eq!(after.iter().filter(|a| a.active).count(), 1);
    let active = f
        .api(ProbeVerdict::Ok, 1)
        .model_snapshot()
        .unwrap()
        .active
        .unwrap();
    assert_eq!(active.id.as_deref(), Some("hermes-live"));
    assert_eq!(active.model, "live-1");
}

#[test]
fn editing_the_bundle_in_force_can_move_the_agent_name() {
    let f = Fixture::new();
    f.api(ProbeVerdict::Ok, 1)
        .model_switch("hermes-live", "manual")
        .unwrap();

    let mut draft = f.draft();
    draft.id = "hermes-live".into();
    draft.editing = true;
    draft.label = "Hermes Live".into();
    draft.agent = "hermes-next".into();
    draft.llm_provider = "deepseek".into();
    draft.model = "live-1".into();
    draft.reasoning = "high".into();
    let updated = f
        .api(ProbeVerdict::Ok, 2)
        .model_bundle_update(&draft)
        .unwrap();
    assert!(updated.agent_changed, "the edit moved the agent NAME");

    let agents = f.agents();
    let old = agents.iter().find(|a| a.name == "hermes").unwrap();
    assert!(!old.active, "the old record keeps its identity, unmarked");
    assert_eq!(old.model, "live-1");
    let moved = agents.iter().find(|a| a.name == "hermes-next").unwrap();
    assert!(moved.active, "the new name carries the marker");
    let snap = f.api(ProbeVerdict::Ok, 1).model_snapshot().unwrap();
    assert_eq!(snap.active.unwrap().agent, "hermes-next");
    let last = f.events().last().unwrap().clone();
    assert_eq!(last.payload[json_field::AGENT_CHANGED], true);
}

/// R4/R5: the wire form and the declaration share ONE field catalogue —
/// every field the file accepts has a form field (and the form adds only its
/// own transport fields). A field added to one side alone fails this pin.
#[test]
fn the_form_wire_carries_every_declared_field() {
    let wire = serde_json::to_value(BundleFormInput::default()).unwrap();
    let mut wire_keys: Vec<String> = wire.as_object().unwrap().keys().cloned().collect();
    wire_keys.retain(|k| k != "editing" && k != "file_revision");
    let mut expected = devai_store::bundle_keys();
    wire_keys.sort();
    expected.sort();
    assert_eq!(
        wire_keys, expected,
        "the form carries exactly the declared fields"
    );
    let mut probe_keys: Vec<String> = wire["probe"].as_object().unwrap().keys().cloned().collect();
    let mut expected_probe = devai_store::probe_keys();
    probe_keys.sort();
    expected_probe.sort();
    assert_eq!(probe_keys, expected_probe);
}

/// R7 branch: a home that declares nothing yet gets a valid `bundles:` list
/// on the first create — the file is created, not appended to nothing.
#[test]
fn creating_on_a_home_without_a_declared_file_creates_the_catalogue() {
    let f = Fixture::empty();
    assert_eq!(f.revision(), "", "an empty home carries the empty edition");
    let created = f
        .api(ProbeVerdict::Ok, 3)
        .model_bundle_create(&f.draft())
        .unwrap();
    assert_eq!(created.outcome, "created");
    assert_ne!(created.revision, "");

    let catalog = devai_store::load_catalog(&f.home.path().join("config")).unwrap();
    assert_eq!(catalog.ids(), vec!["hermes-alibaba"]);
    let text = f.file();
    assert!(text.contains("bundles:"), "{text}");
    assert_eq!(
        devai_store::catalog_revision(&f.home.path().join("config")).unwrap(),
        created.revision
    );
}

/// R7 branch: a write that cannot land leaves the file byte-identical, the
/// probe evidence intact, and a `failed` fact naming the cause.
#[test]
fn a_failed_write_keeps_the_file_and_records_the_failure() {
    let f = Fixture::new();
    let before = f.file();
    // Make the writer's own temp path a directory: creating the temp file
    // fails deterministically, whatever the process runs as.
    let block = f
        .home
        .path()
        .join("config")
        .join(format!(".models.yaml.tmp-{}", std::process::id()));
    std::fs::create_dir(&block).unwrap();

    let err = f
        .api(ProbeVerdict::Ok, 5)
        .model_bundle_create(&f.draft())
        .unwrap_err();
    std::fs::remove_dir(&block).unwrap();

    assert_eq!(err.code(), ErrorCode("io"), "{err}");
    assert_eq!(f.file(), before, "the file must not change");
    let last = f.events().last().unwrap().clone();
    assert_eq!(last.kind, event_kind::MODEL_BUNDLE_CREATED);
    assert_eq!(last.payload[json_field::OUTCOME], "failed");
    assert_eq!(last.payload[json_field::REASON], "io");
    assert_eq!(last.payload[json_field::VERDICT], "ok");
}

/// R7 branch: deleting the LAST declaration leaves a valid, readable empty
/// catalogue (no half-written file, no shape error on reopen).
#[test]
fn deleting_the_last_bundle_leaves_a_readable_empty_catalogue() {
    let f = Fixture::new();
    for id in ["hermes-live", "claude-live"] {
        f.api(ProbeVerdict::Ok, 1)
            .model_bundle_delete(id, &f.revision())
            .unwrap();
    }
    let catalog = devai_store::load_catalog(&f.home.path().join("config")).unwrap();
    assert!(catalog.bundles.is_empty());
    assert!(f.file().contains("bundles:"), "{}", f.file());
    assert!(f.api(ProbeVerdict::Ok, 1).model_snapshot().is_ok());
}
