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

        let mut cmd = std::process::Command::new("claude");
        cmd.arg("-p")
            .arg(&user)
            .arg("--output-format")
            .arg("json")
            .arg("--no-session-persistence")
            // Roles produce formal artifacts from given inputs; they get no
            // tools. File/shell work is a later, explicit capability.
            .arg("--allowedTools")
            .arg("");
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
            "quality-report" => {
                // Mirror requirement ids from the spec we were given as input.
                let crit = req
                    .inputs
                    .iter()
                    .filter(|(l, _)| l.contains("REQUIREMENTS_SPEC"))
                    .find_map(|(_, c)| serde_json::from_str::<Value>(c).ok())
                    .map(|spec| {
                        spec.get("requirements")
                            .and_then(|r| r.as_array())
                            .map(|rs| {
                                rs.iter()
                                    .filter_map(|r| r.get("id").and_then(|i| i.as_str()))
                                    .map(|id| serde_json::json!({
                                        "requirement_id": id,
                                        "status": "PASS",
                                        "evidence": ["mock runtime"],
                                    }))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "verdict": "ACCEPT",
                    "criteria_results": if crit.is_empty() {
                        vec![serde_json::json!({"requirement_id": "R1", "status": "PASS", "evidence": ["mock runtime"]})]
                    } else { crit },
                    "objective_checks": { "build": "n/a (mock)", "tests": "n/a (mock)" },
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

/// Light contract gate: every key in the schema's top-level `required` list
/// must appear in the output. Full JSON-Schema validation is a later step;
/// this keeps garbage from silently entering the artifact store.
fn validate_against_contract(content: &str, schema: Option<&str>) -> Result<(), AdapterError> {
    let Some(schema) = schema else { return Ok(()); };
    let value: Value = serde_json::from_str(content)
        .map_err(|e| AdapterError::BadOutput(format!("output is not valid JSON: {e}")))?;
    if !value.is_object() {
        return Err(AdapterError::BadOutput("output must be a JSON object".into()));
    }
    let schema_v: Value = serde_json::from_str(schema)
        .map_err(|e| AdapterError::BadOutput(format!("stored contract schema is broken: {e}")))?;
    let Some(required) = schema_v.get("required").and_then(|r| r.as_array()) else { return Ok(()); };
    let obj = value.as_object().unwrap();
    let missing: Vec<&str> = required
        .iter()
        .filter_map(|r| r.as_str())
        .filter(|k| !obj.contains_key(*k))
        .collect();
    if !missing.is_empty() {
        return Err(AdapterError::BadOutput(format!("missing required keys: {}", missing.join(", "))));
    }
    Ok(())
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
        "quality-report" => Some(format!(
            "verdict {verdict}, {} criteria checked",
            json.get("criteria_results").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0)
        )),
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
        }
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
    fn mock_quality_mirrors_spec_inputs() {
        let mut r = req("mock", "quality-report");
        r.inputs = vec![(
            "REQUIREMENTS_SPEC v1".to_string(),
            r#"{"requirements":[{"id":"R1"},{"id":"R2"}]}"#.to_string(),
        )];
        let out = AgentRuntime::run(&r).unwrap();
        let v: Value = serde_json::from_slice(&out.content).unwrap();
        assert_eq!(v["criteria_results"].as_array().unwrap().len(), 2);
    }
}
