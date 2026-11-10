//! Starter data for a devai home: roles & criteria come from the
//! declarative config tree (`config/` — see [`crate::config`]), contracts
//! and agents from this module. `devai init` seeds on first run and
//! version-bumps roles when their YAML changes; nothing is overwritten.

use std::path::{Path, PathBuf};

use crate::config;
use crate::{Store, StoreError, StoreResult};
use devai_core::types::*;

/// Insert-only snapshot of the evaluation profile YAMLs into the store, so
/// every recorded `name@version` identity stays resolvable to the exact
/// definition that was in force at execution time — even after an in-place
/// edit bumps the version (the first-stored text for a `(name, version)` IS
/// that version's definition; history is never rewritten).
fn snapshot_profiles(store: &Store, dir: &Path, now: Timestamp) -> StoreResult<usize> {
    let eval_dir = dir.join("evaluation");
    if !eval_dir.is_dir() {
        return Ok(0);
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&eval_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut snapshotted = 0;
    for p in paths {
        let body = std::fs::read_to_string(&p)
            .map_err(|e| StoreError::Config(format!("read {}: {e}", p.display())))?;
        let profile: config::Profile = serde_yaml::from_str(&body)
            .map_err(|e| StoreError::Config(format!("parse {}: {e}", p.display())))?;
        let name = profile
            .name
            .clone()
            .or_else(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()));
        let Some(name) = name else { continue };
        if store.put_evaluation_profile(&name, profile.version, &profile.project, &body, &now)? {
            snapshotted += 1;
        }
    }
    Ok(snapshotted)
}

/// Seed from the home config directory, materializing factory defaults
/// when the tree is absent.
pub fn seed_defaults(store: &Store, now: Timestamp) -> StoreResult<usize> {
    let dir = store.root().join("config");
    config::materialize_if_absent(&dir)?;
    seed_from(store, &dir, now)
}

/// Seed from an explicit config directory (CLI `--config`).
pub fn seed_from(store: &Store, dir: &Path, now: Timestamp) -> StoreResult<usize> {
    let cfg = config::load(dir)?;
    // Validate the loop budget at init time so a typo'd
    // `config/workflow/response-form-loop.yaml` is a LOUD seed error, not a
    // silently ignored policy (the engine itself falls back to factory
    // defaults — the home has been proven here by then).
    config::load_response_form_policy(&dir.join("workflow"))?;
    // REQ-HP-04 AC2: the humanization template is validated with the same
    // loudness — a typo'd key or an inverted volume window fails `devai
    // init`, it is never a silent default.
    config::load_humanize_policy(&dir.join("workflow"))?;
    // REQ-TN-05 AC2: the task-name template is validated the same way —
    // the volume window of the human signboard is config, a broken or
    // inverted declaration fails the seed loudly.
    config::load_task_name_policy(&dir.join("workflow"))?;
    // REQ-AF-05/06: the parking threshold and the anti-spam window are
    // validated at init time — a threshold below 1 or a zero window is a
    // LOUD seed error, never a silently disabled gate.
    config::load_agent_failure_policy(&dir.join("workflow"))?;
    // REQ-AR-14/15/16: the process declaration is validated at init — the
    // closed condition vocabulary, every state's outcome coverage (dead ends
    // refuse init), the brake budgets and the declaration version. A broken
    // or foreign-version declaration never reaches a run.
    config::load_process_policy(&dir.join("workflow"))?;
    let inserted = config::seed_roles(store, &cfg, now)?;

    for (name, version, schema) in contracts() {
        if store.get_contract(name, version)?.is_none() {
            store.put_contract(name, version, &schema)?;
        }
    }

    // Snapshot every evaluation profile definition into the store (INSERT
    // OR IGNORE): a profile identity `name@version` recorded in an old
    // execution must resolve to the exact YAML that was in force then, even
    // after an in-place edit bumps the version. Old rows are never rewritten.
    snapshot_profiles(store, dir, now)?;

    // Default agents. `mock` runs roles offline with deterministic
    // contract-shaped output; `claude` shells out to the headless CLI.
    let existing: Vec<String> = store.list_agents()?.into_iter().map(|a| a.name).collect();
    for (name, provider, model) in [("mock", "mock", ""), ("claude", "claude", "")] {
        if !existing.iter().any(|n| n == name) {
            store.put_agent(&AgentInstance {
                id: new_id(),
                name: name.into(),
                provider: provider.into(),
                model: model.into(),
                runtime: "builtin".into(),
                reasoning: String::new(),
                created_at: now,
                active: false,
            })?;
        }
    }
    Ok(inserted)
}

/// Output-contract schemas (JSON Schema draft-07). These define the SHAPE
/// of role results; the per-check acceptance rules live in
/// `config/criteria/*.yaml`.
pub fn contracts() -> Vec<(&'static str, u32, String)> {
    contracts_raw()
        .into_iter()
        .map(|(name, version, raw)| (name, version, fill_vocabulary(raw)))
        .collect()
}

/// Rework R4: fill the vocabulary placeholders of a schema with the ONE
/// generated vocabulary ([`devai_core::vocab`]). The seeded contract can then
/// never list a check state or item kind the code does not emit, and adding a
/// token is a `vocab.json` edit — not a hunt through hand-copied JSON.
fn fill_vocabulary(raw: &str) -> String {
    raw.replace(
        "@CHECK_STATE_ENUM@",
        devai_core::vocab::CheckState::JSON_ENUM,
    )
    .replace("@ITEM_KIND_LIST@", devai_core::vocab::ItemKind::LIST)
}

fn contracts_raw() -> Vec<(&'static str, u32, &'static str)> {
    vec![
        (
            "requirements-specification",
            1,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Requirements Specification v1",
  "type": "object",
  "required": ["task_id", "version", "business_description", "requirements", "unknowns", "assumptions"],
  "properties": {
    "task_id": { "type": "string" },
    "version": { "type": "integer", "minimum": 1 },
    "source": { "type": "string", "description": "initial description or reference" },
    "requirements": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "statement"],
        "properties": {
          "id": { "type": "string" },
          "statement": { "type": "string" },
          "acceptance_criteria": { "type": "array", "items": { "type": "string" } },
          "constraints": { "type": "array", "items": { "type": "string" } },
          "depends_on": { "type": "array", "items": { "type": "string" } },
          "source": { "type": "object", "properties": { "file": { "type": "string" }, "commit_sha": { "type": "string" }, "checked_at": { "type": "string" } } },
          "unverified": { "type": "boolean" }
        }
      }
    },
    "open_questions": { "type": "array", "items": { "type": "string" } },
    "unknowns": { "type": "array", "items": { "type": "string" } },
    "assumptions": { "type": "array", "items": { "type": "string" } },
    "change_history": { "type": "array", "items": { "type": "object" } }
  }
}"#,
        ),
        (
            // @2 — reply-shape consistency of the SAME contract the gate
            // enforces (REQ-FG-05/06, ratified OQ-FG-03/07). @1 stays seeded
            // as history, insert-only. Fixes @1's hidden expectation
            // `business_description` (required at seed.rs:96 but undeclared
            // in properties — the 3× "$.business_description: missing
            // required key" incident class):
            //  * every required key is now declared in properties (pinned by
            //    a seed audit test over all CURRENT contract versions);
            //  * `task_id` and `business_description` left `required`: the
            //    platform STAMPS `task_id` from the stored task id and
            //    COMPLETES a skipped `business_description` from the stored
            //    description before publishing (REQ-FG-06 / OQ-FG-07 — echo
            //    fields the producer no longer re-types, capture_writing_result
            //    precedent); `version` stays role-owned (rework semantics);
            //  * optional typed fields declare nullability ["x","null"] — the
            //    schema is the single truth for producer AND validator;
            //  * WRITE policy (prompts/common.yaml): omit absent fields, never
            //    write null; READ policy (the subset validator): explicit null
            //    on an OPTIONAL field reads as absence (OQ-FG-03).
            "requirements-specification",
            2,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Requirements Specification v2 — reply shape consistent with the gate",
  "description": "Reply-shape contract of the requirements analyst. WRITE: an optional field you cannot fill is OMITTED entirely — never written as null. READ: the platform gate accepts an explicit null on an optional field as absence and rejects it on a required one. `task_id` is stamped by the platform from stored data (the role does not carry it back); `business_description` is completed from the stored task description when skipped; `version` is role-owned (rework bumps it). Every required key below is declared in `properties` — the schema states exactly what the gate enforces, nothing more.",
  "type": "object",
  "required": ["version", "requirements", "unknowns", "assumptions"],
  "properties": {
    "task_id": { "type": ["string", "null"], "description": "optional in the reply — the platform stamps the stored task id before publishing" },
    "version": { "type": "integer", "minimum": 1 },
    "source": { "type": ["string", "null"], "description": "initial description or reference" },
    "business_description": { "type": ["string", "null"], "description": "optional in the reply — the platform completes it from the stored task description when absent" },
    "requirements": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "statement"],
        "properties": {
          "id": { "type": "string" },
          "statement": { "type": "string" },
          "acceptance_criteria": { "type": ["array", "null"], "items": { "type": "string" } },
          "constraints": { "type": ["array", "null"], "items": { "type": "string" } },
          "depends_on": { "type": ["array", "null"], "items": { "type": "string" } },
          "source": { "type": ["object", "null"], "properties": { "file": { "type": "string" }, "commit_sha": { "type": "string" }, "checked_at": { "type": "string" } } },
          "unverified": { "type": ["boolean", "null"] }
        }
      }
    },
    "open_questions": { "type": ["array", "null"], "items": { "type": "string" } },
    "unknowns": { "type": "array", "items": { "type": "string" } },
    "assumptions": { "type": "array", "items": { "type": "string" } },
    "change_history": { "type": ["array", "null"], "items": { "type": "object" } }
  }
}"#,
        ),
        (
            // @3 — the human paragraph becomes protocol (task 01a1acfe,
            // REQ-HP-01, ratified OQ-HP-1/6). @2 stays seeded as history,
            // insert-only, and stays enforced for artifacts published
            // under it (REQ-HP-06: the obligation is not retroactive).
            // `human_paragraph` is the single ratified field name across
            // all role-borne gate artifacts; presence is enforced here by
            // `required`, edibility by the mechanical detector driven by
            // `config/workflow/humanize.yaml` (REQ-HP-03/04) — the schema
            // deliberately carries NO minLength/maxLength literals, the
            // volume window is config, not code.
            "requirements-specification",
            3,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Requirements Specification v3 — the human paragraph is protocol",
  "description": "Reply-shape contract of the requirements analyst. Same shape as @2 (every required key declared in properties; WRITE: omit optional fields you cannot fill, never null; READ: an explicit null on an optional field reads as absence) plus the ratified OQ-HP-6 obligation: `human_paragraph` is a REQUIRED top-level string — the plain-words explanation the operator reads FIRST, before any formal structure (REQ-HP-05 presentation). Ratified OQ-HP-3: its language follows the task's business description (the template config holds the fallback); OQ-HP-4: on rework (versions 2..N) it is rewritten whole — it explains the CURRENT state, not the version delta. A copy-retelling of the formal body or empty filler is rejected by the deterministic platform detector, never by a model (OQ-HP-2). `task_id` is stamped by the platform from stored data; `business_description` is completed from the stored task description when skipped; `version` is role-owned (rework bumps it).",
  "type": "object",
  "required": ["version", "requirements", "unknowns", "assumptions", "human_paragraph"],
  "properties": {
    "task_id": { "type": ["string", "null"], "description": "optional in the reply — the platform stamps the stored task id before publishing" },
    "version": { "type": "integer", "minimum": 1 },
    "source": { "type": ["string", "null"], "description": "initial description or reference" },
    "business_description": { "type": ["string", "null"], "description": "optional in the reply — the platform completes it from the stored task description when absent" },
    "human_paragraph": { "type": "string", "description": "the human explanation of the artifact, in plain words: what happens now and what will change (required slots, volume window and overlap limits come from config/workflow/humanize.yaml — never restated here)" },
    "requirements": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "statement"],
        "properties": {
          "id": { "type": "string" },
          "statement": { "type": "string" },
          "acceptance_criteria": { "type": ["array", "null"], "items": { "type": "string" } },
          "constraints": { "type": ["array", "null"], "items": { "type": "string" } },
          "depends_on": { "type": ["array", "null"], "items": { "type": "string" } },
          "source": { "type": ["object", "null"], "properties": { "file": { "type": "string" }, "commit_sha": { "type": "string" }, "checked_at": { "type": "string" } } },
          "unverified": { "type": ["boolean", "null"] }
        }
      }
    },
    "open_questions": { "type": ["array", "null"], "items": { "type": "string" } },
    "unknowns": { "type": "array", "items": { "type": "string" } },
    "assumptions": { "type": "array", "items": { "type": "string" } },
    "change_history": { "type": ["array", "null"], "items": { "type": "object" } }
  }
}"#,
        ),
        (
            // @4 — the human task name becomes protocol (task 01a1ad1d,
            // REQ-TN-02). @1..@3 stay seeded as history, insert-only, and
            // stay enforced for artifacts published under them (the same
            // non-retroactivity precedent as @3's paragraph, REQ-TN-02 AC3).
            // `task_name` is the REQUIRED short human signboard the analyst
            // produces from the meaning of THIS spec: the platform stamps it
            // on the task at publication (echo fields `task_id` /
            // `business_description` already travel that path — engine.rs
            // stamp_spec_echo; the name rides the requirements_published
            // command in the SAME commit). Every new CURRENT version brings
            // a new name WHOLE (REQ-TN-04). The volume window and the
            // one-line rule deliberately have NO literals in this schema —
            // they live in `config/workflow/task-name.yaml` and are enforced
            // by the deterministic detector (REQ-TN-05), the ratified
            // humanize idiom.
            "requirements-specification",
            4,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Requirements Specification v4 — the human task name is protocol",
  "description": "Same shape as @3 (every required key declared in properties; WRITE: omit optional fields you cannot fill, never null; READ: an explicit null on an optional field reads as absence) plus the REQ-TN-02 obligation: `task_name` is a REQUIRED top-level string — the short human signboard of the TASK ITSELF, worded from the meaning of this specification (one current name on the task, replaced whole by every new CURRENT version; the formal title of the creator is never rewritten). The volume window, the single-line rule and the field-name discipline live in config/workflow/task-name.yaml and are enforced by the deterministic mechanical detector (REQ-TN-05), never restated as literals in this schema. `human_paragraph` keeps its @3 obligation. `task_id` is stamped by the platform from stored data; `business_description` is completed from the stored task description when skipped; `version` is role-owned (rework bumps it).",
  "type": "object",
  "required": ["version", "requirements", "unknowns", "assumptions", "human_paragraph", "task_name"],
  "properties": {
    "task_id": { "type": ["string", "null"], "description": "optional in the reply — the platform stamps the stored task id before publishing" },
    "version": { "type": "integer", "minimum": 1 },
    "source": { "type": ["string", "null"], "description": "initial description or reference" },
    "business_description": { "type": ["string", "null"], "description": "optional in the reply — the platform completes it from the stored task description when absent" },
    "human_paragraph": { "type": "string", "description": "the human explanation of the artifact, in plain words: what happens now and what will change (required slots, volume window and overlap limits come from config/workflow/humanize.yaml — never restated here)" },
    "task_name": { "type": "string", "description": "the short human signboard of the task, in the language of the business description; the name the queues, the graph and the UI sign the task with (volume window and the single-line rule come from config/workflow/task-name.yaml — never restated here)" },
    "requirements": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "statement"],
        "properties": {
          "id": { "type": "string" },
          "statement": { "type": "string" },
          "acceptance_criteria": { "type": ["array", "null"], "items": { "type": "string" } },
          "constraints": { "type": ["array", "null"], "items": { "type": "string" } },
          "depends_on": { "type": ["array", "null"], "items": { "type": "string" } },
          "source": { "type": ["object", "null"], "properties": { "file": { "type": "string" }, "commit_sha": { "type": "string" }, "checked_at": { "type": "string" } } },
          "unverified": { "type": ["boolean", "null"] }
        }
      }
    },
    "open_questions": { "type": ["array", "null"], "items": { "type": "string" } },
    "unknowns": { "type": "array", "items": { "type": "string" } },
    "assumptions": { "type": "array", "items": { "type": "string" } },
    "change_history": { "type": ["array", "null"], "items": { "type": "object" } }
  }
}"#,
        ),
        (
            "review-report",
            1,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Review Report v1",
  "type": "object",
  "required": ["verdict", "summary", "findings"],
  "properties": {
    "verdict": { "type": "string", "enum": ["APPROVED", "NEEDS_REVISION", "REJECTED"] },
    "summary": { "type": "string" },
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["severity", "description"],
        "properties": {
          "severity": { "type": "string", "enum": ["blocker", "major", "minor", "info"] },
          "description": { "type": "string" },
          "requirement_ref": { "type": "string" }
        }
      }
    }
  }
}"#,
        ),
        (
            // @2 — the human paragraph becomes protocol for the reviewer
            // too (task 01a1acfe, REQ-HP-01, ratified OQ-HP-1/6). @1 stays
            // seeded insert-only and enforced for what was published under
            // it (REQ-HP-06); its `required verdict` precedent — the gate
            // controls protocol mechanically, "не уговорами" — is what the
            // paragraph inherits.
            "review-report",
            2,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Review Report v2 — the human paragraph is protocol",
  "description": "Same shape as @1 (verdict enum, summary, findings) plus the ratified OQ-HP-6 obligation: `human_paragraph` is a REQUIRED top-level string — the plain-words explanation of what the review found and what it means for the task, the first thing the operator reads at the gate (REQ-HP-05). Volume window, explanation slots and overlap limits come from config/workflow/humanize.yaml and are enforced by the deterministic detector (OQ-HP-2), never restated in this schema. On rework the paragraph is rewritten whole (OQ-HP-4).",
  "type": "object",
  "required": ["verdict", "summary", "findings", "human_paragraph"],
  "properties": {
    "verdict": { "type": "string", "enum": ["APPROVED", "NEEDS_REVISION", "REJECTED"] },
    "summary": { "type": "string" },
    "human_paragraph": { "type": "string", "description": "the human explanation of the verdict in plain words: what the review checked, what it found, what happens next" },
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["severity", "description"],
        "properties": {
          "severity": { "type": "string", "enum": ["blocker", "major", "minor", "info"] },
          "description": { "type": "string" },
          "requirement_ref": { "type": "string" }
        }
      }
    }
  }
}"#,
        ),
        (
            "implementation-report",
            1,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Implementation Report v1",
  "type": "object",
  "required": ["branch", "patch", "test_results"],
  "properties": {
    "branch": { "type": "string" },
    "worktree": { "type": "string" },
    "patch": { "type": "string", "description": "artifact id or reference to diff" },
    "test_results": { "type": "object", "description": "real outcomes of build/tests" },
    "requirement_coverage": { "type": "array", "items": { "type": "object" } },
    "notes": { "type": "string" }
  }
}"#,
        ),
        (
            "implementation-report",
            2,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Implementation Report v2 — the platform names branch, worktree and commit",
  "description": "Same required set as @1. `worktree` and `commit` are added for REQ-WT-04: after a writing execution the engine STAMPS `branch` (the platform-created rework branch), `worktree` (the isolation directory the agent worked in) and `commit` (the SHA of the platform commit capturing the work) with ground-truth repository state, then validates the published body. Optional on the agent-reply shape because a read-only-git agent can never know the commit before the platform creates it; the mechanical guarantee is the engine-side stamp + re-validation, pinned in code.",
  "type": "object",
  "required": ["branch", "patch", "test_results"],
  "properties": {
    "branch": { "type": "string", "description": "the platform-created `rework/<task-id>` branch (stamped)" },
    "worktree": { "type": "string", "description": "the isolation worktree directory under DEVAI_HOME the execution ran in (stamped)" },
    "commit": { "type": "string", "description": "full SHA of the resulting platform commit on that branch (stamped)" },
    "patch": { "type": "string", "description": "artifact id or reference to diff" },
    "test_results": { "type": "object", "description": "real outcomes of build/tests" },
    "requirement_coverage": { "type": "array", "items": { "type": "object" } },
    "notes": { "type": "string" }
  }
}"#,
        ),
        (
            // @3 — the human paragraph is protocol on the code-acceptance
            // gate too (task 01a1acfe, ratified OQ-HP-1: the operator reads
            // exactly this artifact when accepting code — it carries the
            // paragraph наравне). @1/@2 stay seeded insert-only (REQ-HP-06).
            // Same stamping discipline as @2: `branch`/`worktree`/`commit`
            // are platform-stamped ground truth; `human_paragraph` is
            // role-owned and must survive the post-stamp re-validation.
            "implementation-report",
            3,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Implementation Report v3 — the human paragraph is protocol",
  "description": "Same required set as @2 plus the ratified OQ-HP-1/6 obligation: `human_paragraph` is a REQUIRED top-level string — the plain-words explanation the operator reads FIRST at the acceptance gate: what was actually done in this run and what changed in the code (REQ-HP-05 presentation; volume window, explanation slots and overlap limits live in config/workflow/humanize.yaml and are enforced by the deterministic detector, never restated here). @2's stamping is unchanged: after a writing execution the engine STAMPS `branch`, `worktree` and `commit` with ground-truth repository state and re-validates the published body — the paragraph written by the role must pass that re-validation untouched (no read-time rewriting, REQ-HP-07).",
  "type": "object",
  "required": ["branch", "patch", "test_results", "human_paragraph"],
  "properties": {
    "branch": { "type": "string", "description": "the platform-created `rework/<task-id>` branch (stamped)" },
    "worktree": { "type": "string", "description": "the isolation worktree directory under DEVAI_HOME the execution ran in (stamped)" },
    "commit": { "type": "string", "description": "full SHA of the resulting platform commit on that branch (stamped)" },
    "patch": { "type": "string", "description": "artifact id or reference to diff" },
    "human_paragraph": { "type": "string", "description": "the human explanation of the implementation in plain words: what happens now and what changed" },
    "test_results": { "type": "object", "description": "real outcomes of build/tests" },
    "requirement_coverage": { "type": "array", "items": { "type": "object" } },
    "notes": { "type": "string" }
  }
}"#,
        ),
        (
            "quality-report",
            2,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Quality Report v2 — machine-assembled by the platform",
  "description": "Assembled by devai-runtime from the evaluation profile: platform-run gates and mechanical probes merged with the auditor's rubric scores. The verdict is computed by the deterministic function of docs/evaluation-schema.md 2.5, never by a model. report_text is the 2.6 human rendering.",
  "type": "object",
  "required": ["verdict", "recommendation_basis", "report_text", "profile", "evaluation"],
  "properties": {
    "verdict": { "type": "string", "enum": ["ACCEPT", "RETURN", "ESCALATE"] },
    "recommendation_basis": { "type": "string" },
    "report_text": { "type": "string" },
    "profile": {
      "type": "object",
      "required": ["project", "version"],
      "properties": { "project": { "type": "string" }, "version": { "type": "integer" } }
    },
    "assessment_execution": { "type": "string", "description": "role execution id of the auditor run" },
    "evaluation": {
      "type": "object",
      "required": ["verdict", "score", "theta", "gates", "checks"],
      "properties": {
        "verdict": { "type": "string", "enum": ["PASS", "FAIL", "INCONCLUSIVE"] },
        "score": { "type": "number" },
        "theta": { "type": "number" },
        "blocked_weight": { "type": "number" },
        "max_blocked_weight": { "type": "number" },
        "floors_breached": { "type": "array", "items": { "type": "string" } },
        "pending_human": { "type": "array", "items": { "type": "string" } },
        "gates": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "state"],
            "properties": {
              "id": { "type": "string" },
              "state": { "type": "string", "enum": ["PASS", "FAIL", "BLOCKED"] },
              "detail": { "type": "string" }
            }
          }
        },
        "checks": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "verify", "state", "weight", "detects"],
            "properties": {
              "id": { "type": "string" },
              "verify": { "type": "string", "enum": ["mechanical", "rubric", "hybrid"] },
              "state": { "type": "string", "enum": @CHECK_STATE_ENUM@ },
              "score": { "type": "number" },
              "floor": { "type": "number" },
              "weight": { "type": "number" },
              "detects": { "type": "array", "items": { "type": "string" } },
              "violations": { "type": "array", "items": { "type": "string" } },
              "evidence": { "type": "array", "items": { "type": "string" } },
              "reason": { "type": "string", "description": "why BLOCKED / unscored" }
            }
          }
        }
      }
    }
  }
}"#,
        ),
        (
            "quality-report",
            3,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Quality Report v3 — machine-assembled by the platform; every check cites evidence",
  "description": "Assembled by devai-runtime from the evaluation profile: platform-run gates and mechanical probes merged with the auditor's rubric scores. The verdict is computed by the deterministic function of docs/evaluation-schema.md 2.5, never by a model. report_text is the 2.6 human rendering. v3 QA rule: every criteria entry in evaluation.checks must cite at least one concrete evidence item (test output, artifact id, build log); a missing or empty evidence array is contract-invalid — no check may be recorded as a pass on model judgment alone.",
  "type": "object",
  "required": ["verdict", "recommendation_basis", "report_text", "profile", "evaluation"],
  "properties": {
    "verdict": { "type": "string", "enum": ["ACCEPT", "RETURN", "ESCALATE"] },
    "recommendation_basis": { "type": "string" },
    "report_text": { "type": "string" },
    "profile": {
      "type": "object",
      "required": ["project", "version"],
      "properties": { "project": { "type": "string" }, "version": { "type": "integer" } }
    },
    "assessment_execution": { "type": "string", "description": "role execution id of the auditor run" },
    "evaluation": {
      "type": "object",
      "required": ["verdict", "score", "theta", "gates", "checks"],
      "properties": {
        "verdict": { "type": "string", "enum": ["PASS", "FAIL", "INCONCLUSIVE"] },
        "score": { "type": "number" },
        "theta": { "type": "number" },
        "blocked_weight": { "type": "number" },
        "max_blocked_weight": { "type": "number" },
        "floors_breached": { "type": "array", "items": { "type": "string" } },
        "pending_human": { "type": "array", "items": { "type": "string" } },
        "gates": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "state"],
            "properties": {
              "id": { "type": "string" },
              "state": { "type": "string", "enum": ["PASS", "FAIL", "BLOCKED"] },
              "detail": { "type": "string" }
            }
          }
        },
        "checks": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "verify", "state", "weight", "detects", "evidence"],
            "properties": {
              "id": { "type": "string" },
              "verify": { "type": "string", "enum": ["mechanical", "rubric", "hybrid"] },
              "state": { "type": "string", "enum": @CHECK_STATE_ENUM@ },
              "score": { "type": "number" },
              "floor": { "type": "number" },
              "weight": { "type": "number" },
              "detects": { "type": "array", "items": { "type": "string" } },
              "violations": { "type": "array", "items": { "type": "string" } },
              "evidence": { "type": "array", "minItems": 1, "items": { "type": "string" }, "description": "concrete citations: test output excerpt, artifact id, build log reference — at least one per check entry" },
              "reason": { "type": "string", "description": "why BLOCKED / unscored" }
            }
          }
        }
      }
    }
  }
}"#,
        ),
        (
            "quality-report",
            4,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Quality Report v4 — machine-assembled by the platform; bound to a workspace revision",
  "description": "Assembled by devai-runtime from the evaluation profile: platform-run gates and mechanical probes merged with the auditor's rubric scores. The verdict is computed by the deterministic function of docs/evaluation-schema.md 2.5, never by a model. v4 = v3 (every criteria entry must cite at least one concrete evidence item) plus the workspace_revision object the platform captured at execution start: the report's findings are traceable to the exact git HEAD + dirty-tree digest + profile name@version + roles slug@version they were made against. A report whose embedded revision equals the freshly recomputed workspace revision is FRESH: re-auditing an unchanged workspace requires an explicit human decision, never a silent second audit.",
  "type": "object",
  "required": ["verdict", "recommendation_basis", "report_text", "profile", "workspace_revision", "evaluation"],
  "properties": {
    "verdict": { "type": "string", "enum": ["ACCEPT", "RETURN", "ESCALATE"] },
    "recommendation_basis": { "type": "string" },
    "report_text": { "type": "string" },
    "profile": {
      "type": "object",
      "required": ["project", "version"],
      "properties": { "project": { "type": "string" }, "version": { "type": "integer" } }
    },
    "workspace_revision": {
      "type": "object",
      "description": "Workspace provenance captured platform-side (never by a model) at execution start. All four components are required; git_head/dirty_digest are null outside a git repo, profile is null when no evaluation profile is loaded — such a revision is not comparable and freshness is never claimed from it.",
      "required": ["git_head", "dirty_digest", "profile", "roles"],
      "properties": {
        "git_head": { "type": ["string", "null"], "description": "git rev-parse HEAD of the probe workspace" },
        "dirty_digest": { "type": ["string", "null"], "description": "sha256 hex over the canonical dirty-tree line sequence: one sorted TAB-separated XY/worktree-oid/index-oid/path line per path reported by `git status --porcelain -z --no-renames --untracked-files=all` (read-only plumbing; a clean tree hashes the empty sequence)" },
        "profile": { "type": ["string", "null"], "description": "evaluation profile identity name@version in force at execution start" },
        "roles": { "type": "array", "items": { "type": "string" }, "description": "sorted slug@version list of the current role definitions at execution start" }
      }
    },
    "assessment_execution": { "type": "string", "description": "role execution id of the auditor run" },
    "evaluation": {
      "type": "object",
      "required": ["verdict", "score", "theta", "gates", "checks"],
      "properties": {
        "verdict": { "type": "string", "enum": ["PASS", "FAIL", "INCONCLUSIVE"] },
        "score": { "type": "number" },
        "theta": { "type": "number" },
        "blocked_weight": { "type": "number" },
        "max_blocked_weight": { "type": "number" },
        "floors_breached": { "type": "array", "items": { "type": "string" } },
        "pending_human": { "type": "array", "items": { "type": "string" } },
        "gates": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "state"],
            "properties": {
              "id": { "type": "string" },
              "state": { "type": "string", "enum": ["PASS", "FAIL", "BLOCKED"] },
              "detail": { "type": "string" }
            }
          }
        },
        "checks": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "verify", "state", "weight", "detects", "evidence"],
            "properties": {
              "id": { "type": "string" },
              "verify": { "type": "string", "enum": ["mechanical", "rubric", "hybrid"] },
              "state": { "type": "string", "enum": @CHECK_STATE_ENUM@ },
              "score": { "type": "number" },
              "floor": { "type": "number" },
              "weight": { "type": "number" },
              "detects": { "type": "array", "items": { "type": "string" } },
              "violations": { "type": "array", "items": { "type": "string" } },
              "evidence": { "type": "array", "minItems": 1, "items": { "type": "string" }, "description": "concrete citations: test output excerpt, artifact id, build log reference — at least one per check entry" },
              "reason": { "type": "string", "description": "why BLOCKED / unscored" }
            }
          }
        }
      }
    }
  }
}"#,
        ),
        (
            "quality-report",
            5,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Quality Report v5 — machine-assembled; the human form is stored with the report",
  "description": "v4 plus `human_view`: the human PRESENTATION of the same report, assembled ONCE by the platform at publication from already-decided values (scores, weights, floors, pending, gates, ratified anchors) and quoted verbatim by every reader — never regrouped, re-scored or re-sorted at read time. Four groups in a fixed order: blocks (the S<theta threshold breach, gates, floor breaches, blocking, pending signature), nuances (every point still below 1.0, each with score, weight and importance = (1-score)*weight, most important first; a fully passed point with no decision link is the norm and is counted in headline.counts.passed instead of being listed), anchors (ONLY positions with an explicit ratified_by decision link, names only), unchecked (BLOCKED checks and unknown states, with reason and weight). Code and probe output live in the per-item evidence layer, never in the visible one-line summary. report_text keeps its v3/v4 §2.6 form: it is the fallback for bodies published before this version, not a second source of truth.",
  "type": "object",
  "required": ["verdict", "recommendation_basis", "report_text", "profile", "workspace_revision", "human_view", "evaluation"],
  "properties": {
    "verdict": { "type": "string", "enum": ["ACCEPT", "RETURN", "ESCALATE"] },
    "recommendation_basis": { "type": "string" },
    "report_text": { "type": "string" },
    "human_view": {
      "type": "object",
      "description": "The stored human form (REQ-QH-01..04). Readers quote it; nothing here is recomputed on read.",
      "required": ["version", "lang", "headline", "groups", "blocked_weight", "max_blocked_weight"],
      "properties": {
        "version": { "type": "integer" },
        "lang": { "type": "string", "description": "language of every human string in this shape (task language, ru)" },
        "headline": {
          "type": "object",
          "required": ["verdict", "machine_verdict", "score", "theta", "basis", "numbers", "counts"],
          "properties": {
            "verdict": { "type": "string", "description": "human verdict word (ПРОШЁЛ / НЕ ПРОШЁЛ / ВЕРДИКТ НЕ ГОТОВ)" },
            "machine_verdict": { "type": "string", "enum": ["PASS", "FAIL", "INCONCLUSIVE"] },
            "score": { "type": "number", "description": "S as decided by the deterministic verdict function at assembly" },
            "theta": { "type": "number", "description": "the verdict threshold the profile scored against" },
            "basis": { "type": "string", "description": "the recommendation basis, quoted from the decided values" },
            "numbers": { "type": "string", "description": "REQUIRED: S and threshold already formatted by the one formatter at assembly; every reader (CLI and panel) prints it verbatim and cannot load the form without it (rework R3/R1)" },
            "counts": {
              "type": "object",
              "required": ["blocks", "nuances", "anchors", "unchecked"],
              "properties": {
                "blocks": { "type": "integer" },
                "nuances": { "type": "integer" },
                "anchors": { "type": "integer" },
                "unchecked": { "type": "integer" },
                "passed": { "type": "integer", "description": "fully passed points (1.0, no decision link): the norm, counted not listed (optional in v1 forms)" }
              }
            }
          }
        },
        "groups": {
          "type": "array",
          "minItems": 4,
          "maxItems": 4,
          "description": "always four, in the fixed order blocks, nuances, anchors, unchecked; an empty group prints no heading",
          "items": {
            "type": "object",
            "required": ["key", "title", "lead", "items"],
            "properties": {
              "key": { "type": "string", "enum": ["blocks", "nuances", "anchors", "unchecked"] },
              "title": { "type": "string" },
              "lead": { "type": "string" },
              "items": {
                "type": "array",
                "items": {
                  "type": "object",
                  "required": ["id", "kind", "summary", "line", "site"],
                  "properties": {
                    "id": { "type": "string", "description": "check id or gate id — the identity the operator cites back" },
                    "kind": { "type": "string", "description": "@ITEM_KIND_LIST@" },
                    "summary": { "type": "string", "description": "ONE short visible line — no code, no paragraphs (REQ-QH-04)" },
                    "line": { "type": "string", "description": "REQUIRED: the COMPLETE visible row (id, score, weight, importance, site — or the anchor sentence), formatted once at assembly; every channel prints it verbatim and neither the CLI nor the panel can load the form without it (rework R3/R1)" },
                    "site": { "type": "string", "description": "file:line of the finding, empty when there is none" },
                    "score": { "type": ["number", "null"], "description": "optional: the decided score; always emitted by the v5 writer, null for items without a score (gates, pending, blocked)" },
                    "weight": { "type": ["number", "null"], "description": "optional: the profile weight; always emitted by the v5 writer, null when the item carries no weight" },
                    "importance": { "type": ["number", "null"], "description": "optional: (1-score)*weight as decided at assembly; always emitted by the v5 writer, null when the item has no score" },
                    "ratified_by": { "type": "string", "description": "optional, OMITTED when absent: the stored decision that keeps this anchor in force" },
                    "reason": { "type": "string", "description": "optional, OMITTED when absent: the BLOCKED reason / gate detail — a fact, not a judgement" },
                    "evidence": { "type": "array", "items": { "type": "string" }, "description": "optional, OMITTED when empty: the collapsed proof layer — citation bodies and probe output, verbatim" }
                  }
                }
              }
            }
          }
        },
        "blocked_weight": { "type": "number" },
        "max_blocked_weight": { "type": "number" }
      }
    },
    "freshness": {
      "type": "object",
      "description": "NOT stored in the body: the export of a reader (REQ-QH-05) beside the shown report — which row is shown, and whether it is still the last word. Assembled at READ time from raw store facts (task phase, stored artifact status, auditor executions); it re-derives no report content. Declared in this contract so the freshness marker has ONE shape owner and the API/panel cannot drift from its required fields (rework R1).",
      "required": ["stale", "marker", "report_seq", "report_status", "task_state"],
      "properties": {
        "stale": { "type": "boolean", "description": "true when the shown row is not the last word and the task is not terminal" },
        "marker": { "type": "string", "description": "ready-to-show marker naming the stored facts; empty when the report is current" },
        "report_seq": { "type": "integer", "description": "seq of the shown artifact row" },
        "report_status": { "type": "string", "description": "stored status of the shown row: CURRENT / STALE / SUPERSEDED" },
        "task_state": { "type": "string", "description": "the task phase the marker names" },
        "execution": { "type": ["string", "null"], "description": "optional: id of the newest auditor execution, null when none ran" },
        "execution_status": { "type": ["string", "null"], "description": "optional: status of that execution (RUNNING is named «в работе» only from this fact)" }
      }
    },
    "profile": {
      "type": "object",
      "required": ["project", "version"],
      "properties": { "project": { "type": "string" }, "version": { "type": "integer" } }
    },
    "workspace_revision": {
      "type": "object",
      "description": "Workspace provenance captured platform-side (never by a model) at execution start. All four components are required; git_head/dirty_digest are null outside a git repo, profile is null when no evaluation profile is loaded — such a revision is not comparable and freshness is never claimed from it.",
      "required": ["git_head", "dirty_digest", "profile", "roles"],
      "properties": {
        "git_head": { "type": ["string", "null"] },
        "dirty_digest": { "type": ["string", "null"] },
        "profile": { "type": ["string", "null"] },
        "roles": { "type": "array", "items": { "type": "string" } }
      }
    },
    "assessment_execution": { "type": "string", "description": "role execution id of the auditor run" },
    "evaluation": {
      "type": "object",
      "required": ["verdict", "score", "theta", "gates", "checks"],
      "properties": {
        "verdict": { "type": "string", "enum": ["PASS", "FAIL", "INCONCLUSIVE"] },
        "score": { "type": "number" },
        "theta": { "type": "number" },
        "blocked_weight": { "type": "number" },
        "max_blocked_weight": { "type": "number" },
        "floors_breached": { "type": "array", "items": { "type": "string" } },
        "pending_human": { "type": "array", "items": { "type": "string" } },
        "gates": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "state"],
            "properties": {
              "id": { "type": "string" },
              "state": { "type": "string", "enum": ["PASS", "FAIL", "BLOCKED"] },
              "detail": { "type": "string" }
            }
          }
        },
        "checks": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["id", "verify", "state", "weight", "detects", "evidence"],
            "properties": {
              "id": { "type": "string" },
              "verify": { "type": "string", "enum": ["mechanical", "rubric", "hybrid"] },
              "state": { "type": "string", "enum": @CHECK_STATE_ENUM@ },
              "score": { "type": "number" },
              "floor": { "type": "number" },
              "weight": { "type": "number" },
              "detects": { "type": "array", "items": { "type": "string" } },
              "violations": { "type": "array", "items": { "type": "string" } },
              "evidence": { "type": "array", "minItems": 1, "items": { "type": "string" }, "description": "concrete citations: test output excerpt, artifact id, build log reference — at least one per check entry" },
              "reason": { "type": "string", "description": "why BLOCKED / unscored" }
            }
          }
        }
      }
    }
  }
}"#,
        ),
        (
            "evaluation-assessment",
            1,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Evaluation Assessment v1 — auditor's raw rubric reply",
  "description": "What the quality-auditor agent emits. Scores only for checks it is allowed to score; no verdict (the platform computes it). Every score below 1.0 must carry evidence or appear in gaps.",
  "type": "object",
  "required": ["scores", "findings", "gaps"],
  "properties": {
    "scores": { "type": "object", "description": "check-id -> anchor score in [0,1]" },
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["check", "text", "evidence"],
        "properties": {
          "check": { "type": "string" },
          "text": { "type": "string", "description": "what + file:line + what it leads to" },
          "evidence": { "type": "array", "items": { "type": "string" } }
        }
      }
    },
    "gaps": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["check", "reason"],
        "properties": { "check": { "type": "string" }, "reason": { "type": "string" } }
      }
    },
    "notes": { "type": "string" }
  }
}"#,
        ),
        (
            "quality-report",
            1,
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Quality Report v1",
  "type": "object",
  "required": ["verdict", "criteria_results"],
  "properties": {
    "verdict": { "type": "string", "enum": ["ACCEPT", "RETURN", "ESCALATE"] },
    "criteria_results": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["requirement_id", "status", "evidence"],
        "properties": {
          "requirement_id": { "type": "string" },
          "status": { "type": "string", "enum": ["PASS", "FAIL", "NOT_CHECKED"] },
          "evidence": { "type": "array", "minItems": 1, "items": { "type": "string" }, "description": "artifact ids / test outputs / build logs — every criteria_results entry must cite at least one concrete piece of evidence" }
        }
      }
    },
    "objective_checks": {
      "type": "object",
      "properties": {
        "build": { "type": "string" },
        "tests": { "type": "string" },
        "lint": { "type": "string" },
        "coverage": { "type": "string" }
      }
    }
  }
}"#,
        ),
    ]
}

/// Rework R3/R5/R8: the schema's state enum and `kind` list are the SAME
/// vocabulary the producer and the human form use (`devai_core::vocab`).
/// Adding a state or a kind without updating every surface fails here — the
/// store schema can no longer drift into describing a smaller or larger set
/// than the code emits.
#[cfg(test)]
mod tests {
    use devai_core::vocab::{CheckState, ItemKind};

    use super::*;

    /// The CURRENT quality-report contract (the highest seeded version).
    fn quality_report_schema() -> serde_json::Value {
        let (_, _, raw) = contracts()
            .into_iter()
            .filter(|(name, _, _)| *name == "quality-report")
            .max_by_key(|(_, version, _)| *version)
            .expect("quality-report contract is seeded");
        serde_json::from_str(&raw).expect("the seeded schema is valid JSON")
    }

    #[test]
    fn the_check_state_enum_is_the_producer_vocabulary() {
        let schema = quality_report_schema();
        let states: Vec<&str> = schema["properties"]["evaluation"]["properties"]["checks"]["items"]
            ["properties"]["state"]["enum"]
            .as_array()
            .expect("checks.state enum exists")
            .iter()
            .map(|v| v.as_str().expect("enum entries are strings"))
            .collect();
        let expected: Vec<&str> = CheckState::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(states, expected);
    }

    #[test]
    fn the_item_kind_list_is_the_form_vocabulary() {
        let schema = quality_report_schema();
        let desc = schema["properties"]["human_view"]["properties"]["groups"]["items"]
            ["properties"]["items"]["items"]["properties"]["kind"]["description"]
            .as_str()
            .expect("kind description exists");
        let listed: Vec<&str> = desc.split(" | ").map(str::trim).collect();
        let expected: Vec<&str> = ItemKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(listed, expected);
    }
}
