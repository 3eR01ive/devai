//! The human paragraph in role artifacts (task 01a1acfe, REQ-HP-01..08).
//! Proof surface: the SEEDED contracts, the single gate on both publication
//! paths, the detector scope, the config-driven thresholds, non-retroactivity
//! and the size discipline. Offline, deterministic, on the seeded home —
//! the same fixtures `engine.rs` uses.

use chrono::Utc;
use devai_core::types::*;
use devai_runtime::{Engine, artifact_violations, validate_reply};
use devai_store::config::TaskNamePolicy;
use devai_store::{Store, seed};
use serde_json::{Value, json};

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    seed::seed_defaults(&store, Utc::now()).unwrap();
    (dir, store)
}

fn contract(store: &Store, name: &str, version: u32) -> String {
    store
        .get_contract(name, version)
        .unwrap()
        .unwrap_or_else(|| panic!("{name}@{version} must be seeded"))
}

/// REQ-HP-01 AC1 — insert-only versions with the paragraph obligation.
#[test]
fn seeded_contracts_carry_the_required_paragraph() {
    let (_d, store) = store();
    let cases = [
        ("requirements-specification", 3),
        ("review-report", 2),
        ("implementation-report", 3),
    ];
    for (name, version) in cases {
        let schema: Value = serde_json::from_str(&contract(&store, name, version)).unwrap();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            required.contains(&"human_paragraph"),
            "{name}@{version} must require the paragraph: {required:?}"
        );
        // The schema-authority invariant: every required key is declared.
        assert!(
            schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("human_paragraph"),
            "{name}@{version} declares the paragraph in properties"
        );
    }
    // AC4 — quality-report gets NO duplicate field: its report_text IS the
    // paragraph slot; @2..@4 remain untouched by this task.
    for v in [2, 3, 4] {
        let schema: Value = serde_json::from_str(&contract(&store, "quality-report", v)).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x == "report_text"),
            "quality-report@{v} keeps report_text required"
        );
        assert!(
            !serde_json::to_string(&schema)
                .unwrap()
                .contains("human_paragraph"),
            "no paragraph field is bolted onto quality-report@{v}"
        );
    }
    // REQ-HP-06 — the history versions stay seeded (insert-only).
    assert!(
        store
            .get_contract("requirements-specification", 2)
            .unwrap()
            .is_some()
    );
    assert!(store.get_contract("review-report", 1).unwrap().is_some());
    assert!(
        store
            .get_contract("implementation-report", 2)
            .unwrap()
            .is_some()
    );
}

/// REQ-HP-01 AC2 — the creator roles moved to the new versions and the
/// reader roles' input refs moved with them (declarative, but it must not
/// drift). OQ-HP-1 (ratified): implementation-agent reads the @3 spec too.
#[test]
fn role_contracts_point_at_the_paragraph_versions() {
    let (_d, store) = store();
    let out = |slug: &str| {
        store
            .get_role(slug)
            .unwrap()
            .unwrap()
            .output_contract
            .unwrap()
            .to_string()
    };
    let inp = |slug: &str| {
        store
            .get_role(slug)
            .unwrap()
            .unwrap()
            .input_contract
            .map(|c| c.to_string())
    };
    // Task 01a1ad1d (REQ-TN-02 AC4): the role half moved to @4 — the
    // paragraph obligation is INHERITED by @4 (it still requires
    // human_paragraph), so "points at the paragraph version" holds at the
    // current version; the name bump must not silently unpin this check.
    assert_eq!(out("requirements-analyst"), "requirements-specification@4");
    // @4 добавляет платформенную ссылку на проверенную спеку (spec_seq /
    // spec_artifact) поверх обязательств @2 — параграф по-прежнему обязателен.
    assert_eq!(out("requirements-reviewer"), "review-report@4");
    assert_eq!(out("implementation-agent"), "implementation-report@3");
    assert_eq!(
        inp("requirements-reviewer").as_deref(),
        Some("requirements-specification@4")
    );
    assert_eq!(
        inp("implementation-agent").as_deref(),
        Some("requirements-specification@4")
    );
    // @3 stays seeded as history with the paragraph required (insert-only).
    let s3: Value =
        serde_json::from_str(&contract(&store, "requirements-specification", 3)).unwrap();
    assert!(
        s3["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "human_paragraph"),
        "@3 keeps the paragraph obligation"
    );
    // …and @4 adds the name on top of it (REQ-TN-01/02: one field, required).
    let s4: Value =
        serde_json::from_str(&contract(&store, "requirements-specification", 4)).unwrap();
    let r4: Vec<&str> = s4["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        r4.contains(&"task_name") && r4.contains(&"human_paragraph"),
        "{r4:?}"
    );
}

/// The detector fixtures as GATE data on the seeded @3 schema: absence is a
/// required-key violation on both paths (engine and manual publish run this
/// one function — REQ-HP-02 parity), unreadable prose is a detector
/// violation, a real paragraph passes. OQ-HP-2 (ratified): strictly
/// mechanical — no provider is reachable from this code path at all.
#[test]
fn the_seeded_gate_rejects_absence_and_chatter_accepts_a_real_paragraph() {
    let (_d, store) = store();
    let schema = contract(&store, "requirements-specification", 3);
    let policy = devai_store::config::HumanizePolicy::default();

    let skeleton = |paragraph: Option<&str>| -> Value {
        let mut v = json!({
            "version": 1,
            "requirements": [{"id": "R1", "statement": "Что-то формальное."}],
            "unknowns": [],
            "assumptions": []
        });
        if let Some(p) = paragraph {
            v["human_paragraph"] = json!(p);
        }
        v
    };

    // 1) No paragraph — the presence half of the gate (REQ-HP-02): the
    //    violation names the field with its json path.
    let v = artifact_violations(
        &skeleton(None).to_string(),
        Some(&schema),
        &policy,
        &TaskNamePolicy::default(),
    )
    .unwrap();
    assert!(
        v.iter()
            .any(|e| e == "$.human_paragraph: missing required key"),
        "absence must be a required-key violation: {v:?}"
    );

    // 2) Empty chatter — detector rejection, FORM-class.
    let v = artifact_violations(
        &skeleton(Some("всё хорошо, работа выполнена")).to_string(),
        Some(&schema),
        &policy,
        &TaskNamePolicy::default(),
    )
    .unwrap();
    assert!(
        v.iter()
            .any(|e| e.starts_with("$.human_paragraph:") && e.contains("minimum")),
        "filler must be rejected: {v:?}"
    );

    // 3) A real paragraph passes the whole gate.
    let good = "Простыми словами: сейчас выводы ролей читаются только специалистом, и человеку на гейте приходится переводить формальный протокол вручную по памяти о запуске. После этого решения каждый артефакт будет начинаться с понятного абзаца: что происходит и что изменится, — а платформа проверит его наличие механически и не пустит гейт без него.";
    let v = artifact_violations(
        &skeleton(Some(good)).to_string(),
        Some(&schema),
        &policy,
        &TaskNamePolicy::default(),
    )
    .unwrap();
    assert_eq!(
        v,
        Vec::<String>::new(),
        "a human paragraph must pass: {v:?}"
    );

    // Parity (REQ-HP-02): the engine half raises the SAME facts as a
    // response_form rejection with the full list — one gate, one function.
    let e = validate_reply(
        &skeleton(None).to_string(),
        Some(&schema),
        &policy,
        &TaskNamePolicy::default(),
    )
    .unwrap_err();
    assert_eq!(e.failure_class(), failure_class::RESPONSE_FORM);
    match e {
        devai_runtime::AdapterError::BadOutput { violations } => assert_eq!(violations, {
            artifact_violations(
                &skeleton(None).to_string(),
                Some(&schema),
                &policy,
                &TaskNamePolicy::default(),
            )
            .unwrap()
        }),
        other => panic!("form class expected: {other}"),
    }
}

/// REQ-HP-03 — the exempt scope is structural: quality-report@4's stored
/// schema carries no paragraph field, so publishing its platform-assembled
/// report_text runs ZERO detector checks on it (an overlap test would
/// reject the §2.6 render by construction — platform-class, never replayed
/// onto a role, docs/response-form-loop.md:19).
#[test]
fn quality_report_text_never_meets_the_detector() {
    let (_d, store) = store();
    let schema = contract(&store, "quality-report", 4);
    let policy = devai_store::config::HumanizePolicy::default();
    // A report_text that WOULD fail the detector as a role field (short,
    // chatter) — exempt: no violation can reference the paragraph or the
    // detector at all.
    let body = json!({
        "verdict": "ACCEPT",
        "recommendation_basis": "all gates passed",
        "report_text": "ок",
        "profile": { "project": "p", "version": 1 },
        "workspace_revision": {
            "git_head": null, "dirty_digest": null, "profile": null, "roles": []
        },
        "evaluation": { "verdict": "PASS", "score": 0.9, "theta": 0.8, "gates": [], "checks": [] }
    });
    let v = artifact_violations(
        &body.to_string(),
        Some(&schema),
        &policy,
        &TaskNamePolicy::default(),
    )
    .unwrap();
    assert!(
        v.iter()
            .all(|e| !e.contains("human_paragraph") && !e.contains("paragraph")),
        "report_text must not meet the detector: {v:?}"
    );
    assert_eq!(
        v,
        Vec::<String>::new(),
        "the assembled @4 shape is valid: {v:?}"
    );
}

/// REQ-HP-04 — thresholds and volume come from the project's config, not
/// from code: the same 50-char paragraph is rejected at the factory minimum
/// and accepted when the project lowers the window. The engine resolves the
/// file exactly like the form budget (absent => factory bytes; a broken
/// file is a HELD loud stop at the next stage, never a silent default).
#[test]
fn volume_and_thresholds_are_config_data_not_code() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("workflow");
    std::fs::create_dir_all(&wf).unwrap();

    // Factory defaults without any project file at all (AC3).
    let absent = devai_store::config::load_humanize_policy(&wf.join("nope")).unwrap();
    assert_eq!(absent.min_chars, 200);
    assert_eq!(absent.max_chars, 1200);
    assert_eq!(absent.field, "human_paragraph");

    std::fs::write(
        wf.join("humanize.yaml"),
        "humanize:\n  min_chars: 40\n  max_chars: 100\n  ngram: 2\n  overlap_max_ratio: 0.95\n  forbidden_markers: []\n  slots: []\n",
    )
    .unwrap();
    let project = devai_store::config::load_humanize_policy(&wf).unwrap();
    assert_eq!(
        (project.min_chars, project.max_chars, project.ngram),
        (40, 100, 2)
    );

    let (_d, store) = store();
    let schema = contract(&store, "requirements-specification", 3);
    let body = json!({
        "version": 1,
        "requirements": [{"id": "R1", "statement": "Требование."}],
        "unknowns": [], "assumptions": [],
        "human_paragraph": "короткий, но уже достаточный для проекта текст абзаца"
    })
    .to_string();
    // Rejected at the factory minimum…
    let strict =
        artifact_violations(&body, Some(&schema), &absent, &TaskNamePolicy::default()).unwrap();
    assert!(
        strict
            .iter()
            .any(|e| e.contains("below the configured minimum")),
        "{strict:?}"
    );
    // …accepted at the project window — no code changed anywhere.
    let lenient =
        artifact_violations(&body, Some(&schema), &project, &TaskNamePolicy::default()).unwrap();
    assert_eq!(lenient, Vec::<String>::new(), "{lenient:?}");

    // Typo'd key: LOUD at load (AC2), naming the file.
    std::fs::write(wf.join("humanize.yaml"), "humanize:\n  min_charss: 40\n").unwrap();
    let e = devai_store::config::load_humanize_policy(&wf)
        .unwrap_err()
        .to_string();
    assert!(e.contains("humanize") && e.contains("min_charss"), "{e}");
}

/// REQ-HP-04/09 + the held-error discipline: a broken humanize.yaml in the
/// engine's config dir stops the NEXT stage loudly, and `devai init`
/// (seed_from) refuses the same file at once.
#[test]
fn broken_template_stops_the_engine_loudly_not_quietly() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(cfg.join("workflow")).unwrap();
    std::fs::write(
        cfg.join("workflow/humanize.yaml"),
        "humanize:\n  min_chars: 900\n  max_chars: 300\n",
    )
    .unwrap();
    // Inverted window: the volume check could never pass — a config BUG,
    // refused at load with the meaning spelled out.
    let e = devai_store::config::load_humanize_policy(&cfg.join("workflow")).unwrap_err();
    assert!(e.to_string().contains("min_chars"), "{e}");

    // `devai init` on such a tree fails LOUD (the seed validator). A full
    // factory tree is materialised first (the seed needs roles/ too), THEN
    // the humanize file is corrupted — so the failure can only come from
    // the validator, not from an unrelated missing file.
    let home = dir.path().join("home");
    let home_store = Store::open(&home).unwrap();
    let cfg_home = home.join("config");
    std::fs::create_dir_all(&cfg_home).unwrap();
    devai_store::config::materialize_if_absent(&cfg_home).unwrap();
    std::fs::write(
        cfg_home.join("workflow/humanize.yaml"),
        "humanize:\n  overlap_max_ratio: 3.0\n",
    )
    .unwrap();
    let err = seed::seed_from(&home_store, &cfg_home, Utc::now()).unwrap_err();
    assert!(
        err.to_string().contains("overlap_max_ratio"),
        "seed must name the broken key: {err}"
    );

    // The engine construction stays infallible; the error is HELD and
    // raised at the first stage (the delivery-table discipline).
    let (sd, store) = store();
    let t = Task::new("engine-stop", "б", None, Utc::now());
    store.create_task(&t).unwrap();
    let engine = Engine::new(&store).with_humanize_config_dir(&cfg.join("workflow"));
    // The file IS present and broken => held error.
    let err = engine
        .advance(&t.id.to_string(), "mock", "tester")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("humanize") || err.contains("min_chars"),
        "{err}"
    );
    drop(sd);
}

/// REQ-HP-06 — non-retroactivity at the gate: the SAME body with no
/// paragraph is LEGAL under the old @2 contract and illegal under @3.
/// Previously CURRENT artifacts are never re-judged by the new rule.
#[test]
fn obligation_binds_the_new_versions_only() {
    let (_d, store) = store();
    let legacy = contract(&store, "requirements-specification", 2);
    let current = contract(&store, "requirements-specification", 3);
    let body = json!({
        "version": 1,
        "requirements": [{"id": "R1", "statement": "Требование."}],
        "unknowns": [], "assumptions": []
    })
    .to_string();
    let policy = devai_store::config::HumanizePolicy::default();
    assert!(
        artifact_violations(&body, Some(&legacy), &policy, &TaskNamePolicy::default())
            .unwrap()
            .is_empty(),
        "a body published under @2 stays legal — the obligation is not retroactive"
    );
    let v =
        artifact_violations(&body, Some(&current), &policy, &TaskNamePolicy::default()).unwrap();
    assert_eq!(
        v,
        vec!["$.human_paragraph: missing required key".to_string()]
    );
}

/// REQ-HP-08 — the size discipline: a paragraph at the FACTORY MAX plus the
/// complete @3 skeleton fits the declared reply limit, parses whole, and
/// passes the gate; a truncated reply (cut at the limit) is a FORM rejection,
/// never a silent pass. The limit constant is declared right here — the
/// proof about the budget never depends on provider internals.
#[test]
fn factory_max_fits_the_reply_limit_and_truncation_is_rejected() {
    const REPLY_LIMIT: usize = 12_000; // declared fixture limit, chars
    let (_d, store) = store();
    let schema = contract(&store, "requirements-specification", 3);
    let policy = devai_store::config::HumanizePolicy::default();

    // A paragraph of exactly the factory max, with both slots and no
    // forbidden markers: a plausible filler sentence, repeated with variety.
    let base = "Сейчас формальные выводы ролей приходится переводить вручную; ";
    let mut paragraph = String::new();
    while paragraph.chars().count() < policy.max_chars - base.chars().count() {
        paragraph.push_str(base);
    }
    paragraph
        .push_str("После принятия решения изменится то, что абзац станет обязанностью протокола.");
    paragraph.truncate(
        paragraph
            .chars()
            .take(policy.max_chars)
            .map(|c| c.len_utf8())
            .sum(),
    );
    assert_eq!(paragraph.chars().count(), policy.max_chars);

    let body = json!({
        "version": 1,
        "requirements": [
            {"id": "R1", "statement": "Абзац обязателен.", "acceptance_criteria": ["гейт без абзаца не проходит"], "constraints": [], "depends_on": []},
            {"id": "R2", "statement": "Пороги живут в конфиге.", "acceptance_criteria": ["правка без пересборки"], "constraints": [], "depends_on": []},
            {"id": "R3", "statement": "Детектор механический.", "acceptance_criteria": ["тот же вход — тот же вердикт"], "constraints": [], "depends_on": []}
        ],
        "unknowns": ["метрика overlap не калибрована"],
        "assumptions": ["дефолты консервативны"],
        "open_questions": [],
        "change_history": [{"version": 1, "summary": "initial"}],
        "human_paragraph": paragraph
    });
    let text = serde_json::to_string(&body).unwrap();
    assert!(
        text.chars().count() <= REPLY_LIMIT,
        "full skeleton + max paragraph must fit the reply limit: {} > {REPLY_LIMIT}",
        text.chars().count()
    );
    // Parses WHOLE at the limit — no eaten tail keys.
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(parsed["human_paragraph"].is_string());
    assert!(parsed.get("change_history").is_some(), "tail keys survive");
    // …and passes the gate (max itself is legal, it is the window boundary).
    let v = artifact_violations(
        &parsed.to_string(),
        Some(&schema),
        &policy,
        &TaskNamePolicy::default(),
    )
    .unwrap();
    assert_eq!(v, Vec::<String>::new(), "exactly-max paragraph: {v:?}");

    // A truncated reply (the incident shape: the budget ate the tail) stays
    // a FORM-class rejection — silence here would re-open the 3× incident.
    let chars: Vec<char> = text.chars().collect();
    let cut: String = chars[..chars.len() - 50].iter().collect();
    let v = artifact_violations(&cut, Some(&schema), &policy, &TaskNamePolicy::default());
    match v {
        Ok(list) => assert!(!list.is_empty(), "truncated body must not pass: {list:?}"),
        Err(devai_runtime::AdapterError::BadOutput { violations }) => {
            assert!(!violations.is_empty());
        }
        Err(other) => panic!("truncated reply is FORM class, got {other}"),
    }
}

/// The engine-path close (REQ-HP-02 AC1): a role reply without the paragraph
/// never reaches the store — the form loop parks/rejects it and NO CURRENT
/// spec artifact appears after the run.
#[test]
fn engine_publishes_no_paragraph_less_spec() {
    let (_d, store) = store();
    let t = Task::new("no-paragraph", "нужен абзац", None, Utc::now());
    store.create_task(&t).unwrap();
    // `mock-form-broken` violates the stored contract on EVERY attempt,
    // whatever the schema — under @3 that includes the paragraph presence.
    // With the loop budget at 0 it is the plain loud rejection: no artifact,
    // no replay of the cycle.
    let r = Engine::new(&store).with_max_repair_attempts(0).advance(
        &t.id.to_string(),
        "mock-form-broken",
        "tester",
    );
    assert!(
        r.is_err(),
        "a form-broken analyst must not complete the stage"
    );
    let arts = store.artifacts(t.id).unwrap();
    assert!(
        !arts
            .iter()
            .any(|a| a.kind == ArtifactKind::RequirementsSpec),
        "rejected bodies never become artifacts"
    );
}
