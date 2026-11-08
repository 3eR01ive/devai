//! Cycle C1 (task 01a1ab88): the project overlay DELTA — representation,
//! force rule (A-5), composition semantics (ADD-only), weight procedure
//! (REQ-SP-107) and per-project resolution (REQ-SP-103). The tests run
//! against a materialised factory home, so the base under composition is
//! the SHIPPED base.yaml, not a fixture that could drift from it.
//!
//! The overlay can only ADD rules under new ids. It cannot rewrite, fill in
//! or restate a base rule: the retired `bind` item kind is not part of the
//! grammar and is rejected by the parser.
//!
//! FIXTURE DISCIPLINE (QA R1, evaluator collision): this repo IS judged by
//! devai itself, whose devai-v11 profile greps `crates/` for network-stack
//! literals. Illustrative probe patterns here must therefore stay NEUTRAL
//! (`fakenet`, `othercrate`, …) — a fixture quoting a real gate's pattern
//! makes that gate FAIL against its own test data.

use chrono::Utc;
use devai_core::types::*;
use devai_store::config::{self, Profile, ProfileOverlay, overlay_path};
use devai_store::{Store, seed};
use std::fs;
use tempfile::TempDir;

fn home() -> (TempDir, Store, std::path::PathBuf) {
    let d = TempDir::new().unwrap();
    let cfg = d.path().join("config");
    config::materialize_if_absent(&cfg).unwrap();
    let store = Store::open(d.path()).unwrap();
    let _ = seed::seed_from(&store, &cfg, Utc::now()).unwrap();
    (d, store, cfg)
}

fn project(store: &Store, name: &str) -> Project {
    let p = Project::new(name, "", vec![], Utc::now());
    store.create_project(&p).unwrap();
    p
}

/// A human ratification row as the load cross-check sees it: kind
/// profile_ratification, verdict APPROVED, refs covering the item ids.
fn ratify(store: &Store, task: &Task, refs: &[&str]) -> Id {
    let id = new_id();
    store
        .record_decision(&Decision {
            id,
            task_id: task.id,
            kind: decision_kind::PROFILE_RATIFICATION.into(),
            verdict: Verdict::Approved,
            actor: "operator".into(),
            rationale: Some("ratified item".into()),
            refs: refs.iter().map(|s| s.to_string()).collect(),
            created_at: Utc::now(),
        })
        .unwrap();
    id
}

fn write_overlay(cfg: &std::path::Path, project_id: &str, body: &str) {
    let p = overlay_path(cfg, project_id);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, body).unwrap();
}

fn task(store: &Store, project: Option<&Project>) -> Task {
    let t = Task::new(
        "overlay cycle",
        "C1 data layer",
        project.map(|p| p.id),
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    t
}

fn base_profile(cfg: &std::path::Path) -> Profile {
    config::load_profiles(&cfg.join("evaluation"))
        .unwrap()
        .into_iter()
        .find(|p| p.name.as_deref() == Some("base"))
        .expect("factory base is present")
}

/// A ratified ADD item body (mechanical probe, neutral fixture pattern).
fn add_item(id: &str, decision: &str, weight: f64) -> String {
    format!(
        r#"  - kind: add
    target_id: {id}
    ratified_by: {decision}
    add:
      statement: local-first — nothing may reach out past the declared adapter.
      detects: [security/injection]
      verify: mechanical
      probes:
        - {{ cmd: "test -z \"$(grep -RnE 'fakenet|othercrate' src)\"", pass_exit: [0] }}
      weight: {weight}
"#
    )
}

/// REQ-SP-101 AC1/AC4: a clean home receives ONLY the base in evaluation/;
/// the factory tree does not seed a profiles/ subdirectory at all.
#[test]
fn clean_home_gets_base_only_and_no_profiles_namespace() {
    let (_d, _s, cfg) = home();
    let entries: Vec<String> = fs::read_dir(cfg.join("evaluation"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["base.yaml".to_string()]);
    assert!(
        !cfg.join("profiles").exists(),
        "DEFAULT_FILES must not distribute profiles/ (REQ-SP-106 constraint)"
    );
    let loaded = config::load_profiles(&cfg.join("evaluation")).unwrap();
    let base = loaded
        .iter()
        .find(|p| p.applies_to_roles.iter().any(|r| r == "quality-auditor"))
        .expect("auditor profile present");
    assert_eq!(base.identity(), "base@1");
}

/// REQ-SP-106 (A-5, C1-without-C3): an overlay whose items reference
/// decisions that do not exist has NO force — resolution yields the pure
/// base and the loader announces every dropped item.
#[test]
fn unratified_overlay_judges_on_pure_base_and_is_announced() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let t = task(&store, Some(&prj));
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &prj.id.to_string(),
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
{}
{}"#,
            add_item("dom/sovereignty", "no-such-decision", 0.30),
            add_item("dom/portability", "also-no-decision", 0.20),
        ),
    );
    let r = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string())).unwrap();
    let eff = r.profile.clone().expect("resolves to a profile");
    let b = base_profile(&cfg);
    // Pure base: same rules, no new checks, weights intact.
    assert_eq!(eff.checks().len(), b.checks().len());
    assert!(eff.gates.is_empty(), "the base carries no gates");
    assert!(!eff.checks().iter().any(|c| c.id == "dom/sovereignty"));
    assert!(!eff.checks().iter().any(|c| c.id == "dom/portability"));
    assert_eq!(eff.scoring.weights, b.scoring.weights);
    assert_eq!(r.warnings.len(), 2, "both items announced: {r:?}");
    assert!(r.warnings.iter().all(|w| w.contains("NOT in force")));
    let _ = t;
}

/// REQ-SP-103/104/106/107 with ratification: the ratified ADD lands under
/// its new id in its (new) section, the weights follow the deterministic
/// formula, and V7 holds on the effective profile. The base rules are
/// untouched — an overlay adds, it never rewrites.
#[test]
fn ratified_overlay_composes_base_plus_added_rule() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let t = task(&store, Some(&prj));
    let dec_add = ratify(&store, &t, &["dom/sovereignty"]).to_string();
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &prj.id.to_string(),
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 3
items:
  - kind: add
    target_id: dom/sovereignty
    ratified_by: {dec_add}
    add:
      statement: local-first — nothing may reach out past the declared adapter.
      detects: [security/injection]
      verify: mechanical
      probes:
        - {{ cmd: "test -z \"$(grep -RnE 'fakenet|othercrate' src)\"", pass_exit: [0] }}
      weight: 0.30
      error_types:
        architecture/sovereignty-leak:
          desc: Прямой сетевой вызов из кода платформы мимо заявленного адаптера.
          severity: major
      section: platform-invariants
      section_title: Продуктовые инварианты
"#
        ),
    );
    let r = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string())).unwrap();
    let eff = r.profile.expect("effective profile");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let base = base_profile(&cfg);

    // Add: new check under its new id in its (new) section + error type merged.
    let added = eff
        .checks()
        .into_iter()
        .find(|c| c.id == "dom/sovereignty")
        .expect("added check present");
    assert_eq!(added.verify, "mechanical");
    assert!(
        eff.evaluations
            .iter()
            .any(|e| e.id == "platform-invariants" && e.checks.len() == 1),
        "new section carried by the add"
    );
    assert!(
        eff.error_types
            .contains_key("architecture/sovereignty-leak")
    );

    // The base rules are exactly as shipped: same ids, same wording.
    for b in base.checks() {
        let e = eff
            .checks()
            .into_iter()
            .find(|c| c.id == b.id)
            .unwrap_or_else(|| panic!("base check `{}` disappeared", b.id));
        assert_eq!(
            e.statement, b.statement,
            "an add must not rewrite `{}`",
            b.id
        );
    }

    // Weights (REQ-SP-107): W=0.30 -> base scaled by 0.70, add keeps 0.30.
    assert!((eff.scoring.weights["sec/injection"] - 0.04 * 0.7).abs() < 1e-12);
    assert!((eff.scoring.weights["pr/spec-fidelity"] - 0.06 * 0.7).abs() < 1e-12);
    assert_eq!(eff.scoring.weights["dom/sovereignty"], 0.30);
    let sum: f64 = eff.scoring.weights.values().sum();
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "V7 holds on the effective profile: {sum}"
    );
    eff.validate().expect("effective profile passes V1-V8");

    // V2: one id set, nothing doubled.
    let mut ids: Vec<&str> = eff.gates.iter().map(|g| g.id.as_str()).collect();
    ids.extend(eff.checks().iter().map(|c| c.id.as_str()));
    let uniq: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(uniq.len(), ids.len());
}

/// REQ-SP-107 AC2: W >= 1 is a loud rejection enumerating the items.
#[test]
fn add_weights_at_or_over_one_reject_the_publication() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let t = task(&store, Some(&prj));
    let d1 = ratify(&store, &t, &["dom/one"]).to_string();
    let d2 = ratify(&store, &t, &["dom/two"]).to_string();
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &prj.id.to_string(),
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - {{kind: add, target_id: dom/one, ratified_by: {d1}, add: {{statement: s, detects: [], verify: rubric, anchors: {{"1.0": a, "0.5": b, "0.0": c}}, weight: 0.6}}}}
  - {{kind: add, target_id: dom/two, ratified_by: {d2}, add: {{statement: s, detects: [], verify: rubric, anchors: {{"1.0": a, "0.5": b, "0.0": c}}, weight: 0.4}}}}
"#
        ),
    );
    let err = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("REQ-SP-107"), "{err}");
    assert!(err.contains("dom/one") && err.contains("dom/two"), "{err}");
}

/// REQ-SP-106 layer (a): structural violations are rejected BEFORE
/// composition — an add colliding with a base id, an item without
/// ratified_by, and a file whose header id mismatches the projects.id key.
#[test]
fn structural_violations_are_rejected_before_composition() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let t = task(&store, Some(&prj));
    let dec = ratify(&store, &t, &["sec/injection"]).to_string();
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &prj.id.to_string(),
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - {{kind: add, target_id: sec/injection, ratified_by: {dec}, add: {{statement: s, detects: [], verify: rubric, anchors: {{"1.0": a, "0.5": b, "0.0": c}}, weight: 0.1}}}}
"#
        ),
    );
    let err = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("collides with an existing base id"), "{err}");

    write_overlay(
        &cfg,
        &prj.id.to_string(),
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - {{kind: add, target_id: dom/no-decision, add: {{statement: s, detects: [], verify: rubric, anchors: {{"1.0": a, "0.5": b, "0.0": c}}, weight: 0.1}}}}
"#
        ),
    );
    let err = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ratified_by"), "{err}");

    // Header id must equal the projects.id the file is keyed by.
    write_overlay(
        &cfg,
        &prj.id.to_string(),
        r#"project_id: "some-other-id"
base_identity: base@1
overlay_version: 1
items: []
"#,
    );
    let err = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not match the key"), "{err}");
}

/// The overlay can only ADD: the retired `bind` item kind is an unknown serde
/// variant now, and the PARSER rejects it before any validation logic runs —
/// no branch for it survives anywhere in the composition.
#[test]
fn bind_kind_is_rejected_at_parse_time() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &pid,
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - kind: bind
    target_id: sec/injection
    ratified_by: whatever
"#
        ),
    );
    let err = config::resolve_effective_profile(&store, &cfg, Some(&pid))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown variant"), "{err}");
    assert!(err.contains("bind"), "{err}");
}

/// REQ-SP-106 AC: the delta is NOT a Profile — the full-Profile loader
/// would reject it, and nothing in composition parses it as one.
#[test]
fn delta_is_not_parseable_as_a_profile() {
    let body = r#"project_id: pid
base_identity: base@1
overlay_version: 1
items: []
"#;
    // A Profile requires version/project/gates/evaluations/scoring — the
    // delta shape has none of them as Profile-shaped keys.
    assert!(
        serde_yaml::from_str::<Profile>(body).is_err(),
        "the delta file must be rejected by the Profile type"
    );
    let as_overlay: ProfileOverlay = serde_yaml::from_str(body).unwrap();
    assert_eq!(as_overlay.base_identity, "base@1");
}

/// REQ-SP-103 AC3/AC4: the same composition is byte-identical across runs
/// and independent of item order; the key is projects.id, so the resolution
/// works even when the project row's NAME changed or does not resolve.
#[test]
fn composition_is_deterministic_and_keyed_by_project_id() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let t = task(&store, Some(&prj));
    let d1 = ratify(&store, &t, &["dom/a"]).to_string();
    let d2 = ratify(&store, &t, &["dom/b"]).to_string();
    let (a, b) = (add_item("dom/a", &d1, 0.2), add_item("dom/b", &d2, 0.3));
    let overlay = |first: bool| {
        let (x, y) = if first { (&a, &b) } else { (&b, &a) };
        format!(
            "project_id: \"{}\"\nbase_identity: base@1\noverlay_version: 1\nitems:\n{x}{y}",
            prj.id
        )
    };
    write_overlay(&cfg, &prj.id.to_string(), &overlay(true));
    let r1 = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string())).unwrap();
    write_overlay(&cfg, &prj.id.to_string(), &overlay(false));
    let r2 = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string())).unwrap();
    let y = |p: &Profile| serde_yaml::to_string(p).unwrap();
    assert_eq!(
        y(r1.profile.as_ref().unwrap()),
        y(r2.profile.as_ref().unwrap()),
        "byte-identical effective profile for the same composition (REQ-SP-103)"
    );
    assert!(r1.warnings.is_empty(), "{:?}", r1.warnings);

    // Resolution keyed by projects.id survives the name being different:
    // an unknown-to-DB id still composes the overlay (name is display-only).
    let ghost_id = new_id().to_string();
    write_overlay(
        &cfg,
        &ghost_id,
        &overlay(true).replace(&prj.id.to_string(), &ghost_id),
    );
    let r3 = config::resolve_effective_profile(&store, &cfg, Some(&ghost_id)).unwrap();
    assert_eq!(
        y(r3.profile.as_ref().unwrap()),
        y(r1.profile.as_ref().unwrap()),
        "projects.name is display-only; the resolution key is the id"
    );
}

/// REQ-SP-103 AC1: a task of project B is never judged by project A's
/// ratified specifics, and a project without an overlay judges on the base
/// — including when the home carries another project's OWNED full profile.
#[test]
fn projects_do_not_borrow_each_others_checks() {
    let (_d, store, cfg) = home();
    let a = project(&store, "alpha");
    let b = project(&store, "beta");
    let t = task(&store, Some(&a));
    let d = ratify(&store, &t, &["dom/a-only"]).to_string();
    let apid = a.id.to_string();
    write_overlay(
        &cfg,
        &a.id.to_string(),
        &format!(
            r#"project_id: "{apid}"
base_identity: base@1
overlay_version: 1
items:
  - {{kind: add, target_id: dom/a-only, ratified_by: {d}, add: {{statement: s, detects: [], verify: rubric, anchors: {{"1.0": a, "0.5": b, "0.0": c}}, weight: 0.2}}}}
"#
        ),
    );
    let eff_b = config::resolve_effective_profile(&store, &cfg, Some(&b.id.to_string()))
        .unwrap()
        .profile
        .unwrap();
    assert!(!eff_b.checks().iter().any(|c| c.id == "dom/a-only"));
    assert_eq!(eff_b.identity(), "base@1");
    let eff_a = config::resolve_effective_profile(&store, &cfg, Some(&a.id.to_string()))
        .unwrap()
        .profile
        .unwrap();
    assert!(eff_a.checks().iter().any(|c| c.id == "dom/a-only"));

    // Legacy full-profile homes: a profile OWNED by another project must
    // not judge this one; the NEUTRAL base (or the legacy first-match when
    // no neutral profile exists) decides. The existing devai v11 home keeps
    // judging devai (operator protection until C5).
    let legacy = TempDir::new().unwrap();
    let lcfg = legacy.path().join("config");
    config::materialize_if_absent(&lcfg).unwrap();
    let lstore = Store::open(legacy.path()).unwrap();
    let devai = project(&lstore, "devai");
    fs::write(
        lcfg.join("evaluation/quality.yaml"),
        // Minimal auditor profile OWNED by devai (project: devai).
        r#"version: 11
project: devai
applies_to_roles: [quality-auditor]
error_types:
  correctness/state-transition: { desc: d, severity: catastrophic }
gates:
  - id: build-green
    probes: [{ cmd: "cargo build", pass_exit: [0] }]
evaluations:
  - id: platform-invariants
    title: Продуктовые инварианты
    checks:
      - id: corr/legacy-only
        statement: devai invariant
        detects: [correctness/state-transition]
        verify: rubric
        anchors: {"1.0": a, "0.5": b, "0.0": c}
scoring:
  theta: 0.8
  floors: { catastrophic: 0.7 }
  blocked_policy: { max_blocked_weight: 0.2 }
  weights: { corr/legacy-only: 1.0 }
"#,
    )
    .unwrap();
    let other = project(&lstore, "outsider");
    let eff_other = config::resolve_effective_profile(&lstore, &lcfg, Some(&other.id.to_string()))
        .unwrap()
        .profile
        .unwrap();
    assert_eq!(
        eff_other.identity(),
        "base@1",
        "an outsider project must not run under devai's owned checks"
    );
    assert!(
        !eff_other
            .checks()
            .iter()
            .any(|c| c.id == "corr/legacy-only")
    );
    let eff_devai = config::resolve_effective_profile(&lstore, &lcfg, Some(&devai.id.to_string()))
        .unwrap()
        .profile
        .unwrap();
    assert!(
        eff_devai
            .checks()
            .iter()
            .any(|c| c.id == "corr/legacy-only"),
        "the v11 working profile keeps judging devai until C5"
    );
    let _ = t;
}

/// A `base_identity` whose version drifted keeps its ratified adds in force:
/// the base is matched by NAME, only the items' own force rule (A-5) decides
/// what enters composition.
#[test]
fn base_version_drift_keeps_ratified_adds() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let t = task(&store, Some(&prj));
    let d1 = ratify(&store, &t, &["dom/a"]).to_string();
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &prj.id.to_string(),
        &format!(
            r#"project_id: "{pid}"
base_identity: base@99
overlay_version: 1
items:
{}
"#,
            add_item("dom/a", &d1, 0.25)
        ),
    );
    let r = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string())).unwrap();
    let eff = r.profile.unwrap();
    assert_eq!(eff.identity(), "base@1");
    assert!(
        eff.checks().iter().any(|c| c.id == "dom/a"),
        "a version drift in base_identity must not discard a ratified add"
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(
        r.ratified.get("dom/a").map(|(d, _)| d.as_str()),
        Some(d1.as_str())
    );
}

/// A task WITHOUT a project id judges on the pure base of a clean home
/// (REQ-SP-103 AC2).
#[test]
fn task_without_project_judges_on_the_base() {
    let (_d, store, cfg) = home();
    let t = task(&store, None);
    let r = config::resolve_effective_profile(&store, &cfg, None).unwrap();
    assert_eq!(r.profile.unwrap().identity(), "base@1");
    let _ = t;
}

/// R3 (QA): `kind` is a serde ENUM — an invalid value is rejected by the
/// PARSER, before any validation-order logic could run (the old String+match
/// let the check depend on call sequence).
#[test]
fn invalid_kind_is_rejected_at_parse_time() {
    let (_d, store, cfg) = home();
    let prj = project(&store, "alpha");
    let pid = prj.id.to_string();
    write_overlay(
        &cfg,
        &pid,
        &format!(
            r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - {{kind: delete, target_id: sec/injection, ratified_by: whatever}}
"#
        ),
    );
    let err = config::resolve_effective_profile(&store, &cfg, Some(&pid))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown variant"), "{err}");
    assert!(err.contains("delete"), "{err}");
}

/// A home that still carries an older owned full profile keeps judging its
/// projectless tasks: the MOST MATURE auditor court wins over the freshly
/// seeded factory base, and the base stays court only for a clean home
/// (REQ-SP-101).
#[test]
fn projectless_resolution_prefers_the_mature_court_over_the_fresh_base() {
    let (_d, store, cfg) = home();
    // devai-home shape: the pre-split home already carries the owned court
    // (init never clobbers existing files, so quality.yaml survives next to
    // the newly seeded base).
    let src = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/evaluation/quality.yaml"
    );
    fs::copy(src, cfg.join("evaluation").join("quality.yaml")).unwrap();
    let res = config::resolve_effective_profile(&store, &cfg, None).unwrap();
    let profile = res.profile.expect("projectless home still yields a court");
    assert_eq!(
        profile.name.as_deref(),
        Some("quality"),
        "projectless tasks must be judged on the mature court, not the fresh base"
    );
    assert!(profile.version > 1);
}

/// REQ-BIND-04 (C5 D1, the court-unchanged guarantee of the binding slice):
/// binding a task to the devai project moves NO court. The owned-path
/// resolution without an overlay hands the bound task the SAME profile the
/// projectless fallback hands today — byte-for-byte. No overlay file exists
/// in this slice (REQ-BIND-05: D1 creates no profiles/), so both addresses
/// land on quality.yaml.
#[test]
fn bound_task_is_judged_byte_identical_to_the_projectless_fallback() {
    let (_d, store, cfg) = home();
    let src = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/evaluation/quality.yaml"
    );
    fs::copy(src, cfg.join("evaluation").join("quality.yaml")).unwrap();
    // The project row of the devai home: the owned-profile match works by
    // projects.name == profile.project.
    let prj = project(&store, "devai");
    let projectless = config::resolve_effective_profile(&store, &cfg, None)
        .unwrap()
        .profile
        .expect("a devai-shaped home yields the bound court even projectless");
    let bound = config::resolve_effective_profile(&store, &cfg, Some(&prj.id.to_string()))
        .unwrap()
        .profile
        .expect("the bound task resolves a court too");
    assert_eq!(bound.name.as_deref(), Some("quality"));
    assert_eq!(bound.version, projectless.version);
    // Byte-for-byte over the serialized resolved profiles: binding changed
    // nothing the auditor sees — the transition rules of REQ-BIND-04 hold.
    assert_eq!(
        serde_json::to_vec(&bound).unwrap(),
        serde_json::to_vec(&projectless).unwrap(),
        "binding a task must not move its court"
    );
}
