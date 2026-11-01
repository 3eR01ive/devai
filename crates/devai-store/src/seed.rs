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
              "state": { "type": "string", "enum": ["MECHANICAL_PASS", "MECHANICAL_FAIL", "SCORED", "BLOCKED", "PENDING_HUMAN"] },
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
              "state": { "type": "string", "enum": ["MECHANICAL_PASS", "MECHANICAL_FAIL", "SCORED", "BLOCKED", "PENDING_HUMAN"] },
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
