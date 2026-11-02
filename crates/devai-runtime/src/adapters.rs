//! Pluggable agent adapters.
//!
//! A role execution receives an [`ExecRequest`] (role + inputs + contract)
//! and must return contract-shaped JSON. Providers are ordinary data on the
//! `AgentInstance`: `mock` runs fully offline (tests, dry runs), `claude`
//! shells out to the headless `claude` CLI. The adapter is an *interface* —
//! it cannot decide the workflow: the engine validates every transition
//! through `devai-core` afterwards.

use devai_core::types::{Id, RoleDefinition};
use serde_json::Value;

use crate::prompt::{build_system_prompt, build_user_prompt, extract_json_object};

#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub task_id: Id,
    pub title: String,
    pub business: String,
    pub role: RoleDefinition,
    pub provider: String,
    pub model: String,
    /// Input artifacts as `(label, content)` pairs, ordered by dependency.
    pub inputs: Vec<(String, String)>,
    /// JSON Schema text of the role's output contract, when defined.
    pub contract_schema: Option<String>,
    /// Directory the agent process runs in. For a writing role this is the
    /// platform-created isolation WORKTREE, never the main checkout
    /// (REQ-WT-02 AC1); for read-only roles the declared project workspace.
    pub workspace: Option<std::path::PathBuf>,
    /// Platform-declared run-artifact directory for this execution
    /// (`DEVAI_HOME/evidence/<task>/<execution>`), exported to the agent
    /// process as `DEVAI_EVIDENCE_DIR`. Run artifacts (logs, evidence files)
    /// go here and never into the source tree (REQ-WT-05). Location is
    /// platform-owned, not agent-chosen.
    pub evidence: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    /// Contract-shaped JSON bytes — becomes the artifact body verbatim.
    pub content: Vec<u8>,
    pub summary: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("unknown agent provider `{0}` (expected `mock` or `claude`)")]
    UnknownProvider(String),
    #[error("failed to launch agent process: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent error: {0}")]
    Agent(String),
    #[error("agent output does not satisfy the contract: {0}")]
    BadOutput(String),
}

pub struct AgentRuntime;

impl AgentRuntime {
    pub fn run(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        match req.provider.as_str() {
            "mock" => Self::mock(req),
            "claude" => Self::claude(req),
            other => Err(AdapterError::UnknownProvider(other.to_string())),
        }
    }

    // -----------------------------------------------------------------
    // claude: headless CLI call, text -> contract JSON
    // -----------------------------------------------------------------

    fn claude(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        let system = build_system_prompt(&req.role);
        let contract = req.role.output_contract.as_ref().map(|c| (c.name.as_str(), c.version));
        let user = build_user_prompt(&req.title, &req.business, &req.inputs, contract, req.contract_schema.as_deref());

        // The role's declarative `tools` (config/roles/*.yaml) decide what the
        // provider process may touch. No tools declared == pure text answer.
        let tools = claude_tools_for(&req.role);
        let mut cmd = std::process::Command::new("claude");
        cmd.arg("-p")
            .arg(&user)
            .arg("--output-format")
            .arg("json")
            .arg("--no-session-persistence");
        if tools.is_empty() {
            cmd.arg("--allowedTools").arg("");
        } else {
            cmd.arg("--allowedTools").args(&tools);
        }
        if let Some(ws) = &req.workspace {
            cmd.current_dir(ws);
        }
        // REQ-WT-05: the run-artifact sink is platform-declared and exported
        // to the process; the agent never chooses where its logs go.
        if let Some(ev) = &req.evidence {
            cmd.env("DEVAI_EVIDENCE_DIR", ev);
        }
        if !system.is_empty() {
            cmd.arg("--append-system-prompt").arg(system);
        }
        if !req.model.is_empty() {
            cmd.arg("--model").arg(&req.model);
        }

        let out = cmd.output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(AdapterError::Agent(format!("`claude` exited with {}: {}", out.status, err.trim())));
        }
        let envelope: Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| AdapterError::Agent(format!("cannot parse `claude` json envelope: {e}")))?;
        if envelope.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false) {
            let msg = envelope.get("result").and_then(|v| v.as_str()).unwrap_or("unknown agent error");
            return Err(AdapterError::Agent(msg.to_string()));
        }
        let text = envelope
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::Agent("envelope has no `result` field".into()))?;
        let json = extract_json_object(text)
            .ok_or_else(|| AdapterError::BadOutput("agent reply contains no JSON object".into()))?;
        let parsed: Value = serde_json::from_str(&json)
            .map_err(|e| AdapterError::BadOutput(format!("extracted object is not valid JSON: {e}")))?;
        validate_against_contract(&json, req.contract_schema.as_deref())?;
        Ok(ExecOutput { content: json.into_bytes(), summary: summarize(contract.map(|c| c.0), &parsed) })
    }

    // -----------------------------------------------------------------
    // mock: deterministic, offline, contract-valid outputs
    // -----------------------------------------------------------------

    fn mock(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        let name = req.role.output_contract.as_ref().map(|c| c.name.as_str()).unwrap_or("other");
        let json: Value = match name {
            "requirements-specification" => serde_json::json!({
                "task_id": req.task_id.to_string(),
                "version": 1,
                "source": "mock runtime",
                "business_description": req.business,
                "requirements": [{
                    "id": "R1",
                    "statement": format!("Deliver: {}", one_line(&req.title)),
                    "acceptance_criteria": ["The produced result satisfies the business description."],
                    "constraints": [],
                    "depends_on": [],
                }],
                "open_questions": [],
                "unknowns": ["mock runtime: unknowns not analysed"],
                "assumptions": ["The business description is complete enough for a minimal cycle."],
                "change_history": [],
            }),
            "review-report" => serde_json::json!({
                "verdict": "APPROVED",
                "summary": format!("mock review of `{}`: no contradictions found", one_line(&req.title)),
                "findings": [{ "severity": "info", "description": "Generated by the mock runtime." }],
            }),
            "implementation-report" => serde_json::json!({
                "branch": "devai/mock",
                "patch": "mock://no-op",
                "test_results": { "status": "PASSED", "detail": "mock runtime executed no real tests" },
                "notes": format!("mock implementation of {}", one_line(&req.title)),
            }),
            "quality-report" | "evaluation-assessment" => {
                // The QA agent answers with an evaluation-assessment; the
                // platform (engine) assembles the machine quality-report@4.
                // The mock never fabricates rubric scores offline — its gap
                // record becomes the cited evidence on the assembled checks.
                serde_json::json!({
                    "scores": {},
                    "findings": [],
                    "gaps": [{ "check": "*", "reason": "mock runtime: offline assessment performs no rubric scoring" }],
                })
            }
            "decision-record" => serde_json::json!({
                "verdict": "ACCEPT",
                "rationale": "mock aggregation: all audits passed",
            }),
            _ => serde_json::json!({ "mock": true, "role": req.role.slug }),
        };
        let text = serde_json::to_vec_pretty(&json).map_err(|e| AdapterError::Agent(e.to_string()))?;
        validate_against_contract(&String::from_utf8_lossy(&text), req.contract_schema.as_deref())
            .map_err(|e| AdapterError::Agent(format!("mock output invalid (seed/schema drift): {e}")))?;
        Ok(ExecOutput { content: text, summary: summarize(Some(name), &json) })
    }
}

/// Translate a role's declarative capability list (`tools:` in
/// `config/roles/<slug>.yaml`) into `claude` CLI tool syntax. This is the
/// provider DRIVER translation, not policy: which capabilities a role has is
/// config; how `claude` spells them is here. Unknown capabilities are ignored
/// — least privilege by construction.
fn claude_tools_for(role: &RoleDefinition) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in &role.tools {
        match cap.as_str() {
            "filesystem:read" => out.extend(["Read", "Grep", "Glob"].iter().map(|s| s.to_string())),
            "filesystem:read-write" => out.extend(
                ["Read", "Grep", "Glob", "Edit", "Write"].iter().map(|s| s.to_string()),
            ),
            "git" => out.extend(
                ["Bash(git status:*)", "Bash(git diff:*)", "Bash(git log:*)"]
                    .iter()
                    .map(|s| s.to_string()),
            ),
            "shell:build" => out.push("Bash(cargo build:*)".into()),
            "shell:test" => out.push("Bash(cargo test:*)".into()),
            "shell:lint" => out.push("Bash(cargo clippy:*)".into()),
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Contract gate: a JSON-Schema draft-07 subset enforced RECURSIVELY —
/// `required` at every object level, `items` schemas through arrays,
/// `minItems` cardinality on arrays, `enum` membership and primitive type
/// checks, with a json path on each violation. Full JSON-Schema is still out
/// of scope, but garbage cannot silently enter the artifact store any more
/// (defect R2 of the dogfood task). `minItems` is what makes the QA rule
/// "every criteria entry cites concrete evidence" (born in `quality-report@3`,
/// carried unchanged into @4) mechanically enforced, not just declared.
pub fn validate_against_contract(content: &str, schema: Option<&str>) -> Result<(), AdapterError> {
    let Some(schema) = schema else { return Ok(()); };
    let value: Value = serde_json::from_str(content)
        .map_err(|e| AdapterError::BadOutput(format!("output is not valid JSON: {e}")))?;
    if !value.is_object() {
        return Err(AdapterError::BadOutput("output must be a JSON object".into()));
    }
    let schema_v: Value = serde_json::from_str(schema)
        .map_err(|e| AdapterError::BadOutput(format!("stored contract schema is broken: {e}")))?;
    let mut errs = Vec::new();
    validate_node(&schema_v, &value, "$", &mut errs);
    if !errs.is_empty() {
        return Err(AdapterError::BadOutput(errs.join("; ")));
    }
    Ok(())
}

fn validate_node(schema: &Value, value: &Value, path: &str, errs: &mut Vec<String>) {
    let Some(s) = schema.get("type").and_then(|t| t.as_str()) else { return; };
    let Some(obj) = schema.as_object() else { return; };
    match s {
        "object" => {
            let Some(map) = value.as_object() else {
                errs.push(format!("{path}: expected object, got {}", json_kind(value)));
                return;
            };
            if let Some(required) = obj.get("required").and_then(|r| r.as_array()) {
                for k in required.iter().filter_map(|r| r.as_str()) {
                    if !map.contains_key(k) {
                        errs.push(format!("{path}.{k}: missing required key"));
                    }
                }
            }
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                for (k, v) in map {
                    if let Some(sub) = props.get(k) {
                        validate_node(sub, v, &format!("{path}.{k}"), errs);
                    }
                }
            }
        }
        "array" => {
            let Some(items) = value.as_array() else {
                errs.push(format!("{path}: expected array, got {}", json_kind(value)));
                return;
            };
            if let Some(min) = obj.get("minItems").and_then(|m| m.as_u64()) {
                if (items.len() as u64) < min {
                    errs.push(format!("{path}: array has {} item(s), minimum {min}", items.len()));
                }
            }
            if let Some(sub) = obj.get("items") {
                if sub.get("type").is_some() {
                    for (i, it) in items.iter().enumerate() {
                        validate_node(sub, it, &format!("{path}[{i}]"), errs);
                    }
                }
            }
        }
        "string" => {
            let Some(v) = value.as_str() else {
                errs.push(format!("{path}: expected string, got {}", json_kind(value)));
                return;
            };
            if let Some(en) = obj.get("enum").and_then(|e| e.as_array()) {
                if !en.iter().any(|x| x.as_str() == Some(v)) {
                    errs.push(format!("{path}: `{v}` not in enum {en:?}"));
                }
            }
        }
        "integer" => {
            if value.as_i64().is_none() {
                errs.push(format!("{path}: expected integer, got {}", json_kind(value)));
            }
        }
        "number" => {
            if value.as_f64().is_none() {
                errs.push(format!("{path}: expected number, got {}", json_kind(value)));
            }
        }
        "boolean" => {
            if value.as_bool().is_none() {
                errs.push(format!("{path}: expected boolean, got {}", json_kind(value)));
            }
        }
        _ => {}
    }
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn summarize(name: Option<&str>, json: &Value) -> Option<String> {
    let verdict = json.get("verdict").and_then(|v| v.as_str()).unwrap_or("");
    match name.unwrap_or("") {
        "requirements-specification" => Some(format!(
            "{} requirement(s), {} open question(s)",
            json.get("requirements").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0),
            json.get("open_questions").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0),
        )),
        "review-report" => Some(format!("verdict {verdict}: {}", json.get("summary").and_then(|s| s.as_str()).unwrap_or(""))),
        "implementation-report" => Some(format!("branch {}", json.get("branch").and_then(|b| b.as_str()).unwrap_or("?"))),
        "quality-report" => match json.get("evaluation") {
            Some(ev) => Some(format!(
                "{verdict} (evaluation {}, S={:.3})",
                ev.get("verdict").and_then(|v| v.as_str()).unwrap_or("?"),
                ev.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0),
            )),
            None => Some(format!(
                "assessment: {} score(s), {} finding(s), {} gap(s)",
                json.get("scores").and_then(|s| s.as_object()).map(|o| o.len()).unwrap_or(0),
                json.get("findings").and_then(|f| f.as_array()).map(|a| a.len()).unwrap_or(0),
                json.get("gaps").and_then(|g| g.as_array()).map(|a| a.len()).unwrap_or(0),
            )),
        },
        _ => json.get("summary").and_then(|s| s.as_str()).map(|s| s.to_string()),
    }
}

fn one_line(s: &str) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 80 { format!("{}…", t.chars().take(80).collect::<String>()) } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_core::types::{ContractRef, new_id};

    fn role(slug: &str, contract: &str) -> RoleDefinition {
        RoleDefinition {
            id: new_id(),
            slug: slug.into(),
            version: 1,
            name: slug.into(),
            purpose: "test".into(),
            instructions: "test".into(),
            tools: vec![],
            input_contract: None,
            output_contract: Some(ContractRef { name: contract.into(), version: 1 }),
            done_criteria: "test".into(),
            created_at: chrono::Utc::now(),
        }
    }

    fn req(provider: &str, contract: &str) -> ExecRequest {
        ExecRequest {
            task_id: new_id(),
            title: "Minimal process".into(),
            business: "Make devai run its own roles".into(),
            role: role("x", contract),
            provider: provider.into(),
            model: "".into(),
            inputs: vec![],
            contract_schema: None,
            workspace: None,
            evidence: None,
        }
    }

    #[test]
    fn declarative_role_tools_map_to_provider_syntax() {
        let mut r = role("quality-auditor", "quality-report");
        r.tools = vec!["git".into(), "shell:test".into(), "filesystem:read".into(), "api:whatever".into()];
        let t = claude_tools_for(&r);
        assert!(t.contains(&"Bash(cargo test:*)".to_string()));
        assert!(t.contains(&"Bash(git diff:*)".to_string()));
        assert!(t.contains(&"Grep".to_string()));
        // Unknown capability grants nothing; no write/edit tools ever leak in.
        assert!(!t.iter().any(|x| x.contains("Write") || x.contains("Edit") || x.contains("whatever")));
        // Empty declaration == fully closed, as before.
        assert!(claude_tools_for(&role("x", "review-report")).is_empty());
        // read-write capability: the implementer must be able to edit at all.
        r.tools = vec!["filesystem:read-write".into(), "shell:build".into()];
        let t = claude_tools_for(&r);
        assert!(t.contains(&"Edit".to_string()) && t.contains(&"Write".to_string()));
        assert!(t.contains(&"Bash(cargo build:*)".to_string()));
    }

    #[test]
    fn mock_outputs_are_contract_shaped() {
        for c in ["requirements-specification", "review-report", "implementation-report", "quality-report"] {
            let out = AgentRuntime::run(&req("mock", c)).unwrap();
            let v: Value = serde_json::from_slice(&out.content).unwrap();
            assert!(v.is_object(), "mock `{c}` must emit a JSON object");
            assert!(out.summary.is_some());
        }
    }

    #[test]
    fn unknown_provider_is_rejected() {
        assert!(matches!(AgentRuntime::run(&req("telepathy", "review-report")), Err(AdapterError::UnknownProvider(_))));
    }

    #[test]
    fn required_keys_are_enforced() {
        let schema = r#"{"type":"object","required":["verdict","summary"]}"#;
        let mut r = req("mock", "review-report");
        r.contract_schema = Some(schema.into());
        // mock review has both keys
        assert!(AgentRuntime::run(&r).is_ok());
        // a broken schema check: feed a payload missing `verdict` through claude-path validation fn directly
        assert!(matches!(validate_against_contract(r#"{"summary":"x"}"#, Some(schema)), Err(AdapterError::BadOutput(_))));
    }

    #[test]
    fn mock_quality_emits_honest_assessment() {
        let out = AgentRuntime::run(&req("mock", "quality-report")).unwrap();
        let v: Value = serde_json::from_slice(&out.content).unwrap();
        // Mock never fabricates rubric scores offline.
        assert!(v["scores"].as_object().unwrap().is_empty());
        assert_eq!(v["gaps"].as_array().unwrap().len(), 1);
        assert_eq!(v["findings"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn validator_enforces_nested_required_and_types() {
        let schema = r#"{
            "type": "object",
            "required": ["verdict", "evaluation"],
            "properties": {
                "verdict": { "type": "string", "enum": ["ACCEPT", "RETURN"] },
                "evaluation": {
                    "type": "object",
                    "required": ["score", "gates"],
                    "properties": {
                        "score": { "type": "number" },
                        "gates": { "type": "array", "items": { "type": "object", "required": ["id", "state"] } }
                    }
                }
            }
        }"#;
        // Deeply valid.
        let good = r#"{"verdict":"ACCEPT","evaluation":{"score":0.9,"gates":[{"id":"build-green","state":"PASS"}]}}"#;
        assert!(validate_against_contract(good, Some(schema)).is_ok());
        // Missing key TWO levels down.
        let shallow_missing = r#"{"verdict":"ACCEPT","evaluation":{"score":0.9,"gates":[{"id":"build-green"}]}}"#;
        let e = validate_against_contract(shallow_missing, Some(schema)).unwrap_err().to_string();
        assert!(e.contains("gates[0].state"), "{e}");
        // Enum violated at the top.
        let bad_enum = r#"{"verdict":"MAYBE","evaluation":{"score":0.9,"gates":[]}}"#;
        let e = validate_against_contract(bad_enum, Some(schema)).unwrap_err().to_string();
        assert!(e.contains("not in enum"), "{e}");
        // Type washed away.
        let bad_type = r#"{"verdict":"ACCEPT","evaluation":{"score":"0.9","gates":[]}}"#;
        let e = validate_against_contract(bad_type, Some(schema)).unwrap_err().to_string();
        assert!(e.contains("$.evaluation.score: expected number"), "{e}");
    }

    /// QA rule "evidence per criterion", enforced on the SEEDED schema texts:
    /// every `evaluation.checks` entry must carry a non-empty evidence array.
    #[test]
    fn quality_report_gate_requires_evidence_on_every_check() {
        let schema_v3 = devai_store::seed::contracts()
            .into_iter()
            .find(|(name, ver, _)| *name == "quality-report" && *ver == 3)
            .expect("quality-report@3 is seeded")
            .2
            .to_string();
        let good = serde_json::json!({
            "verdict": "ACCEPT",
            "recommendation_basis": "шлюзы зелёные, полы целы",
            "report_text": "ВЕРДИКТ: ПРОШЁЛ",
            "profile": { "project": "devai", "version": 2 },
            "evaluation": {
                "verdict": "PASS", "score": 0.93, "theta": 0.8,
                "gates": [{ "id": "build-green", "state": "PASS", "detail": "`cargo build --workspace` -> exit 0" }],
                "checks": [{
                    "id": "corr/transitions", "verify": "mechanical", "state": "MECHANICAL_PASS",
                    "score": 1.0, "weight": 0.12, "detects": ["correctness/state-transition"],
                    "evidence": ["`cargo test -p devai-core --test workflow` -> exit 0: 14 passed"]
                }]
            }
        });
        assert!(validate_against_contract(&good.to_string(), Some(&schema_v3)).is_ok(), "report citing evidence on every check passes the gate");

        // Missing `evidence` key on a check entry.
        let mut missing = good.clone();
        missing["evaluation"]["checks"][0].as_object_mut().unwrap().remove("evidence");
        let e = validate_against_contract(&missing.to_string(), Some(&schema_v3)).unwrap_err().to_string();
        assert!(e.contains("$.evaluation.checks[0].evidence: missing required key"), "{e}");

        // Empty evidence array — the core of the rule.
        let mut empty = good.clone();
        empty["evaluation"]["checks"][0]["evidence"] = serde_json::json!([]);
        let e = validate_against_contract(&empty.to_string(), Some(&schema_v3)).unwrap_err().to_string();
        assert!(e.contains("$.evaluation.checks[0].evidence: array has 0 item(s), minimum 1"), "{e}");
    }

    /// Legacy wording of the same rule: `criteria_results` under @1.
    #[test]
    fn legacy_quality_report_v1_gate_requires_evidence_per_criterion() {
        let schema_v1 = devai_store::seed::contracts()
            .into_iter()
            .find(|(name, ver, _)| *name == "quality-report" && *ver == 1)
            .expect("quality-report@1 stays seeded for history")
            .2
            .to_string();
        let good = serde_json::json!({
            "verdict": "ACCEPT",
            "criteria_results": [{ "requirement_id": "R1", "status": "PASS", "evidence": ["artifact a1b2c3 — test output excerpt"] }]
        });
        assert!(validate_against_contract(&good.to_string(), Some(&schema_v1)).is_ok());

        let mut empty = good.clone();
        empty["criteria_results"][0]["evidence"] = serde_json::json!([]);
        let e = validate_against_contract(&empty.to_string(), Some(&schema_v1)).unwrap_err().to_string();
        assert!(e.contains("$.criteria_results[0].evidence: array has 0 item(s), minimum 1"), "{e}");

        let mut missing = good.clone();
        missing["criteria_results"][0].as_object_mut().unwrap().remove("evidence");
        let e = validate_against_contract(&missing.to_string(), Some(&schema_v1)).unwrap_err().to_string();
        assert!(e.contains("$.criteria_results[0].evidence: missing required key"), "{e}");
    }
}
