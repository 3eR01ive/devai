//! Starter data for a devai home: roles & criteria come from the
//! declarative config tree (`config/` — see [`crate::config`]), contracts
//! and agents from this module. `devai init` seeds on first run and
//! version-bumps roles when their YAML changes; nothing is overwritten.

use std::path::Path;

use crate::config;
use crate::{Store, StoreResult};
use devai_core::types::*;

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
    let inserted = config::seed_roles(store, &cfg, now)?;

    for (name, version, schema) in contracts() {
        if store.get_contract(name, version)?.is_none() {
            store.put_contract(name, version, schema)?;
        }
    }

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
                created_at: now,
            })?;
        }
    }
    Ok(inserted)
}

/// Output-contract schemas (JSON Schema draft-07). These define the SHAPE
/// of role results; the per-check acceptance rules live in
/// `config/criteria/*.yaml`.
pub fn contracts() -> Vec<(&'static str, u32, &'static str)> {
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
          "evidence": { "type": "array", "items": { "type": "string" }, "description": "artifact ids / test outputs / build logs" }
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
