//! Human ratification of PROJECT RULES (task 01a1bacf, RV1..RV10) through
//! the API surface, with the loader's composition checked on the same home.
//!
//! The scenarios are the spec's own acceptance walk:
//!   1. a home WITHOUT ratifications: the fresh rules file loads, every item
//!      is announced as out of force, the project is judged on the base;
//!   2. after the human `profile.ratify`: the covered items enter force, the
//!      decision carries actor/time/rationale/items/edition, the history
//!      cites it, `task decisions` shows it;
//!   3. after an EDIT of the rules content: force is gone until a NEW
//!      ratification; restoring the exact content returns it;
//!   4. revoking a ratification is a separate human row; the file is not
//!      rewritten; a second revocation refuses;
//!   5. a ratification of project A gives no force to project B.

use std::fs;

use devai_api::{Api, ProfileRatifyReq, ProfileRevokeReq, TaskCreateReq};
use devai_core::types::{decision_kind, event_kind, Project};
use devai_store::config;
use tempfile::TempDir;

fn base_profile() -> &'static str {
    r#"version: 1
name: base
project: ""
applies_to_roles: [quality-auditor]
error_types:
  correctness/state-transition: { desc: base error, severity: catastrophic }
gates: []
evaluations:
  - id: base-rules
    title: Базовые правила
    checks:
      - id: base/one
        statement: базовое правило
        detects: [correctness/state-transition]
        verify: rubric
        anchors: {"1.0": a, "0.5": b, "0.0": c}
scoring:
  theta: 0.8
  floors: { catastrophic: 0.7 }
  blocked_policy: { max_blocked_weight: 0.2 }
  weights: { base/one: 1.0 }
"#
}

fn overlay_body(pid: &str, first: &str, second: &str) -> String {
    format!(
        r#"project_id: "{pid}"
base_identity: base@1
overlay_version: 1
items:
  - kind: add
    target_id: dom/one
    ratified_by: {first}
    add:
      statement: первое проектное правило
      detects: []
      verify: rubric
      anchors: {{"1.0": a, "0.5": b, "0.0": c}}
      weight: 0.2
  - kind: add
    target_id: dom/two
    ratified_by: {second}
    add:
      statement: второе проектное правило
      detects: []
      verify: rubric
      anchors: {{"1.0": a, "0.5": b, "0.0": c}}
      weight: 0.1
"#
    )
}

struct Home {
    dir: TempDir,
    api: Api,
    project: Project,
    task: devai_core::types::Id,
    rules: std::path::PathBuf,
}

/// A home with a factory base, ONE registered project, its rules file and a
/// carrier task. Decisions are per-task rows by storage design, so the
/// ratification names a carrier explicitly (ASM-2) — never derived.
fn home() -> Home {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config");
    fs::create_dir_all(cfg.join("evaluation")).unwrap();
    fs::write(cfg.join("evaluation/base.yaml"), base_profile()).unwrap();
    let api = Api::open(dir.path(), "guerrero").expect("open api");
    // `profile.ratify` resolves the project through the API operation; the
    // project row is created through the same public surface.
    let project = api
        .project_add(devai_api::ProjectAddReq {
            name: "alpha".into(),
            description: String::new(),
            repos: vec![],
        })
        .expect("project add");
    let task = api
        .task_create(TaskCreateReq {
            title: "правила проекта".into(),
            project_id: Some(project.id),
            description: String::new(),
            parent: vec![],
            blocked_by: vec![],
            idempotency_key: None,
        })
        .expect("task create")
        .task
        .id;
    let rules = config::overlay_path(&cfg, &project.id.to_string());
    fs::create_dir_all(rules.parent().unwrap()).unwrap();
    fs::write(&rules, overlay_body(&project.id.to_string(), "", "")).unwrap();
    Home {
        dir,
        api,
        project,
        task,
        rules,
    }
}

fn ratify(h: &Home, items: &[&str]) -> devai_api::ProfileRatifyOutcome {
    h.api
        .profile_ratify(ProfileRatifyReq {
            task: h.task.to_string(),
            project: h.project.id.to_string(),
            items: items.iter().map(|s| s.to_string()).collect(),
            all: false,
            rationale: Some("правила проекта проверены и принимаются".into()),
        })
        .expect("human ratification")
}

/// The loader's verdict on the same home (a second read handle).
fn effective(h: &Home) -> config::Resolution {
    let store = devai_store::Store::open(h.dir.path()).expect("open store");
    config::resolve_effective_profile(
        &store,
        &h.dir.path().join("config"),
        Some(&h.project.id.to_string()),
    )
    .expect("resolution")
}

/// RV8 + RV9: a FRESH rules file is not a load failure. Every item is out of
/// force and announced; the state view prints the current edition.
#[test]
fn fresh_rules_file_loads_with_every_item_announced() {
    let h = home();
    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    assert_eq!(s.items.len(), 2);
    assert!(s.items.iter().all(|i| !i.in_force));
    assert!(s.items.iter().all(|i| i.reason.is_some()));
    assert!(s.edition.starts_with("sha256:"), "{}", s.edition);

    let r = effective(&h);
    assert_eq!(r.profile.expect("base still judges").identity(), "base@1");
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    assert!(r.warnings.iter().all(|w| w.contains("NOT in force")));

    // No decision was written by looking: the view is read-only (RV9).
    assert!(h.api.task_decisions(&h.task.to_string()).unwrap().is_empty());
}

/// RV1/RV5: one human call records ONE decision with the computed edition,
/// the covered items, the actor and the rationale; the covered items enter
/// force and the history/decision views carry the act.
#[test]
fn human_ratification_records_the_act_and_gives_the_items_force() {
    let h = home();
    let out = ratify(&h, &["dom/one"]);
    assert!(out.recorded_now);
    assert_eq!(out.decision.kind, decision_kind::PROFILE_RATIFICATION);
    assert_eq!(out.decision.verdict.as_str(), "APPROVED");
    assert_eq!(out.decision.actor, "guerrero");
    assert_eq!(out.decision.refs, vec!["dom/one".to_string()]);
    assert_eq!(out.edition, out.decision.payload["edition"]);

    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    let one = s.items.iter().find(|i| i.target_id == "dom/one").unwrap();
    assert!(one.in_force, "the covered item must be in force");
    assert_eq!(one.actor.as_deref(), Some("guerrero"));
    let two = s.items.iter().find(|i| i.target_id == "dom/two").unwrap();
    assert!(!two.in_force, "an item outside the list stays out (RV2)");

    // The rules file carries the pointer now — no manual edit required.
    let text = fs::read_to_string(&h.rules).unwrap();
    assert!(text.contains(&out.decision.id.to_string()), "{text}");

    // The loader composes the covered add into the effective profile.
    let r = effective(&h);
    let eff = r.profile.expect("effective profile");
    assert!(eff.checks().iter().any(|c| c.id == "dom/one"));
    assert!(!eff.checks().iter().any(|c| c.id == "dom/two"));

    // History (RV5): an append-only event cites the decision id.
    let events = h.api.task_events(&h.task.to_string(), None).unwrap();
    assert!(
        events.iter().any(|e| e.kind == event_kind::PROFILE_RATIFIED
            && e.payload["decision"] == serde_json::json!(out.decision.id.to_string())),
        "{events:?}"
    );
    // `task decisions` shows it with actor/rationale/refs + payload.
    let decisions = h.api.task_decisions(&h.task.to_string()).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].payload["items"], serde_json::json!(["dom/one"]));
    // RV5: ratifying rules is NOT approving the spec.
    assert!(
        decisions
            .iter()
            .all(|d| d.kind != decision_kind::REQUIREMENTS_APPROVAL)
    );
    assert_eq!(
        h.api.task_get(&h.task.to_string()).unwrap().state,
        devai_core::types::TaskState::Draft
    );

    // RV1 AC6: the same edition + item set reuses the stored decision.
    let again = ratify(&h, &["dom/one"]);
    assert!(!again.recorded_now);
    assert_eq!(again.decision.id, out.decision.id);
    assert_eq!(
        h.api.task_decisions(&h.task.to_string()).unwrap().len(),
        1,
        "a repeat must not add a second row"
    );
}

/// RV1 refusals: the reason is mandatory and an unknown item is named.
#[test]
fn missing_rationale_and_unknown_items_refuse() {
    let h = home();
    let err = h
        .api
        .profile_ratify(ProfileRatifyReq {
            task: h.task.to_string(),
            project: h.project.id.to_string(),
            items: vec!["dom/one".into()],
            all: false,
            rationale: None,
        })
        .unwrap_err();
    assert!(format!("{err}").contains("--rationale"), "{err}");

    let err = h
        .api
        .profile_ratify(ProfileRatifyReq {
            task: h.task.to_string(),
            project: h.project.id.to_string(),
            items: vec!["dom/future".into()],
            all: false,
            rationale: Some("почему".into()),
        })
        .unwrap_err();
    let text = format!("{err}");
    assert!(text.contains("dom/future"), "{text}");
    assert!(h.api.task_decisions(&h.task.to_string()).unwrap().is_empty());
}

/// RV2: editing the rules CONTENT after ratification takes force away and
/// names the reason; restoring the exact content returns it without a new
/// decision.
#[test]
fn editing_the_rules_invalidates_force_until_a_new_ratification() {
    let h = home();
    ratify(&h, &["dom/one"]);
    // The file AS IT STANDS after the act (the pointers written by it).
    let before = fs::read_to_string(&h.rules).unwrap();
    assert_eq!(
        effective(&h)
            .profile
            .unwrap()
            .checks()
            .iter()
            .filter(|c| c.id == "dom/one")
            .count(),
        1
    );

    // Change the WEIGHT — content, not a pointer (RV2 AC1).
    fs::write(&h.rules, before.replace("weight: 0.2", "weight: 0.25")).unwrap();
    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    let one = s.items.iter().find(|i| i.target_id == "dom/one").unwrap();
    assert!(!one.in_force);
    assert_eq!(one.reason.as_deref(), Some("редакция изменена после ратификации"));
    let r = effective(&h);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("dom/one") && w.contains("редакция изменена")),
        "{:?}",
        r.warnings
    );
    assert!(!r.profile.unwrap().checks().iter().any(|c| c.id == "dom/one"));

    // Restore the EXACT ratified content: force is back, no new decision.
    fs::write(&h.rules, &before).unwrap();
    let count = h.api.task_decisions(&h.task.to_string()).unwrap().len();
    assert!(
        effective(&h)
            .profile
            .unwrap()
            .checks()
            .iter()
            .any(|c| c.id == "dom/one")
    );
    assert_eq!(
        h.api.task_decisions(&h.task.to_string()).unwrap().len(),
        count,
        "restoring the same edition must not mint a decision"
    );
}

/// RV2 AC4: rewriting ONLY the ratification pointers does not create a new
/// edition — an unrelated pointer edit leaves force untouched.
#[test]
fn pointer_only_edits_do_not_change_the_edition() {
    let h = home();
    let out = ratify(&h, &["dom/one"]);
    let without_pointer = fs::read_to_string(&h.rules).unwrap();
    // Replace the written id with something else: the digest strips pointers.
    let edited = without_pointer.replace(&out.decision.id.to_string(), "PENDING");
    fs::write(&h.rules, &edited).unwrap();
    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    assert_eq!(s.edition, out.edition, "the edition must not follow a pointer");
    // The pointer itself is now unconfirmed, so the item is out of force.
    let one = s.items.iter().find(|i| i.target_id == "dom/one").unwrap();
    assert!(!one.in_force);
}

/// RV3: a half-applied act (decision present, pointer not) is NOT force;
/// re-running the same command finishes it without minting a second row.
#[test]
fn a_half_applied_act_converges_on_retry() {
    let h = home();
    let out = ratify(&h, &["dom/one"]);
    // Simulate the lost POINTER half: the decision exists, the file's pointer
    // no longer names it (same edition — pointers are stripped).
    let ratified = fs::read_to_string(&h.rules).unwrap();
    fs::write(
        &h.rules,
        ratified.replace(&out.decision.id.to_string(), "PENDING"),
    )
    .unwrap();
    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    assert!(!s.items[0].in_force, "the decision alone gives no force (RV3)");

    let retry = ratify(&h, &["dom/one"]);
    assert!(!retry.recorded_now);
    assert_eq!(retry.decision.id, out.decision.id);
    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    assert!(s.items.iter().find(|i| i.target_id == "dom/one").unwrap().in_force);
    assert_eq!(
        h.api.task_decisions(&h.task.to_string()).unwrap().len(),
        1,
        "the retry must not create a second decision"
    );
}

/// RV6: revocation is a separate human row citing the ratification; the file
/// stays byte-identical; a second revocation refuses; a new ratification of
/// the same edition restores force.
#[test]
fn revocation_takes_force_away_without_touching_the_file() {
    let h = home();
    let out = ratify(&h, &["dom/one"]);
    let bytes_before = fs::read(&h.rules).unwrap();

    let rev = h
        .api
        .profile_revoke(ProfileRevokeReq {
            decision: out.decision.id.to_string(),
            rationale: Some("правило отозвано оператором".into()),
        })
        .expect("revocation");
    assert_eq!(rev.decision.verdict.as_str(), "REJECTED");
    assert_eq!(rev.decision.refs, vec![out.decision.id.to_string()]);
    assert_eq!(rev.decision.payload["revokes"], serde_json::json!(out.decision.id.to_string()));
    assert_eq!(
        fs::read(&h.rules).unwrap(),
        bytes_before,
        "the rules file must not be rewritten by a revocation"
    );

    let s = h.api.profile_state(&h.project.id.to_string()).unwrap();
    let one = s.items.iter().find(|i| i.target_id == "dom/one").unwrap();
    assert!(!one.in_force);
    assert_eq!(one.reason.as_deref(), Some("ратификация отозвана"));
    let r = effective(&h);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("dom/one") && w.contains("ратификация отозвана")),
        "{:?}",
        r.warnings
    );

    let err = h
        .api
        .profile_revoke(ProfileRevokeReq {
            decision: out.decision.id.to_string(),
            rationale: Some("ещё раз".into()),
        })
        .unwrap_err();
    assert!(format!("{err}").contains("already revoked"), "{err}");

    // A new ratification of the same edition returns force (RV6 AC5).
    let again = ratify(&h, &["dom/one"]);
    assert!(again.recorded_now);
    assert_ne!(again.decision.id, out.decision.id);
    assert!(effective(&h)
        .profile
        .unwrap()
        .checks()
        .iter()
        .any(|c| c.id == "dom/one"));
}

/// RV7: same item ids in two projects — a ratification of A never gives
/// force in B, and the unknown project/file refuses by name.
#[test]
fn ratification_is_bound_to_one_project() {
    let h = home();
    let other = h
        .api
        .project_add(devai_api::ProjectAddReq {
            name: "beta".into(),
            description: String::new(),
            repos: vec![],
        })
        .unwrap();
    let cfg = h.dir.path().join("config");
    fs::write(
        config::overlay_path(&cfg, &other.id.to_string()),
        overlay_body(&other.id.to_string(), "", ""),
    )
    .unwrap();
    let out = ratify(&h, &["dom/one"]);
    assert_eq!(out.project_id, h.project.id.to_string());

    let sa = h.api.profile_state(&h.project.id.to_string()).unwrap();
    assert!(sa.items.iter().find(|i| i.target_id == "dom/one").unwrap().in_force);
    let sb = h.api.profile_state(&other.id.to_string()).unwrap();
    let one_b = sb.items.iter().find(|i| i.target_id == "dom/one").unwrap();
    assert!(!one_b.in_force, "project B must not borrow A's ratification");

    let err = h
        .api
        .profile_ratify(ProfileRatifyReq {
            task: h.task.to_string(),
            project: "no-such-project".into(),
            items: vec!["dom/one".into()],
            all: false,
            rationale: Some("почему".into()),
        })
        .unwrap_err();
    assert!(format!("{err}").contains("no-such-project"), "{err}");
}

/// RV1 AC3: the `--all` form covers every item of the current edition and
/// still stores the full explicit list.
#[test]
fn all_items_form_stores_the_explicit_list() {
    let h = home();
    let out = h
        .api
        .profile_ratify(ProfileRatifyReq {
            task: h.task.to_string(),
            project: h.project.id.to_string(),
            items: vec![],
            all: true,
            rationale: Some("вся редакция".into()),
        })
        .unwrap();
    assert_eq!(out.items, vec!["dom/one".to_string(), "dom/two".to_string()]);
    assert_eq!(out.decision.payload["items"], serde_json::json!(["dom/one", "dom/two"]));
    let eff = effective(&h).profile.unwrap();
    assert!(eff.checks().iter().any(|c| c.id == "dom/one"));
    assert!(eff.checks().iter().any(|c| c.id == "dom/two"));
}
