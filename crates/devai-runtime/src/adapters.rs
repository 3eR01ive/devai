//! Pluggable agent adapters.
//!
//! A role execution receives an [`ExecRequest`] (role + inputs + contract)
//! and must return contract-shaped JSON. Providers are ordinary data on the
//! `AgentInstance`: `mock` runs fully offline (tests, dry runs), `claude`
//! shells out to the headless `claude` CLI, `hermes` shells out to the
//! headless Hermes CLI (`-z` one-shot, bare final text on stdout — task
//! 01a19dca, the second live driver proving the role/contract layer is
//! provider-agnostic). The adapter is an *interface* — it cannot decide
//! the workflow: the engine validates every transition through `devai-core`
//! afterwards.

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
    /// LLM-provider override SOURCE for drivers that spell one (ratified
    /// OQ-4): the engine fills it from `AgentInstance.runtime`, because
    /// `provider` already carries the DRIVER name (`hermes`, not the LLM
    /// behind it) and `model` carries the model. Empty — or the neutral
    /// `devai agent add` default `local` — means NO override: the driver
    /// leaves the provider choice to the agent's own user config, where
    /// the keys live (R3: keys stay out of the store; OQ-5: the platform
    /// never forces `--ignore-user-config`).
    pub provider_override: String,
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
    /// Response-form correction loop context (REQ-FG-02 AC2): on attempt N>1
    /// the ORIGINAL task context is kept by the unchanged fields above and
    /// this carries `(attempt, full violation list of attempt N-1)`, rendered
    /// as a loud repair section into the prompt. The role process is
    /// sessionless (`--no-session-persistence`) — what the repair prompt does
    /// not carry, the role cannot know (constraint of REQ-FG-02).
    pub repair: Option<(u32, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    /// Contract-shaped JSON bytes — becomes the artifact body verbatim.
    pub content: Vec<u8>,
    pub summary: Option<String>,
}

/// The two protocol halves of a failed role reply (REQ-FG-01, ASM-3):
/// `BadOutput` is a RESPONSE-FORM rejection — the reply did not satisfy the
/// stored reply-shape contract, a cheap fix the SAME role can make;
/// everything else is a provider/process or platform failure the form loop
/// must not consume attempts on (REQ-FG-03, ratified OQ-FG-02/04).
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The list names the REAL live drivers. `mock-form-*` / `mock-spec-*`
    /// are offline protocol fixtures, deliberately not spelled out here
    /// (they are not operator-selectable drivers — "Not seeded as agents
    /// anywhere", see `AgentRuntime::run`). R6: the text stopped lying once
    /// a second live driver existed.
    #[error("unknown agent provider `{0}` (expected `mock`, `claude` or `hermes`)")]
    UnknownProvider(String),
    #[error("failed to launch agent process: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent error: {0}")]
    Agent(String),
    /// Form rejection with the FULL structured violation list — every json
    /// path, not truncated to the first (REQ-FG-01 AC2). The joined string
    /// stays in the Display for humans and stored row `error` text; the Vec
    /// is what the loop replays to the role and events carry as data.
    #[error("agent output does not satisfy the contract: {}", .violations.join("; "))]
    BadOutput { violations: Vec<String> },
    /// The PLATFORM's own artifact: a broken stored schema, an assembly the
    /// platform produced and its gate rejects. Never replayed to the role
    /// (REQ-FG-03 AC2) — the role is not guilty of a shape the platform
    /// assembled; retrying it would spend role cycles on a platform bug.
    #[error("platform contract/assembly error: {0}")]
    Platform(String),
}

impl AdapterError {
    /// The stored `failure_class` for this error (REQ-FG-01/03): form vs
    /// provider vs platform, as data on the execution row — an operator
    /// answers "form rejection or role failure?" without parsing text.
    pub fn failure_class(&self) -> &'static str {
        match self {
            AdapterError::BadOutput { .. } => devai_core::types::failure_class::RESPONSE_FORM,
            AdapterError::Agent(_) | AdapterError::Io(_) | AdapterError::UnknownProvider(_) => devai_core::types::failure_class::PROVIDER,
            AdapterError::Platform(_) => devai_core::types::failure_class::PLATFORM,
        }
    }
}

pub struct AgentRuntime;

/// The engine implements this to HOST the adapter's form-correction loop
/// (OQ-FG-06, ratified: the loop lives in the adapter, which owns reply
/// shape; the host owns the store rows and events, which the adapter must
/// never touch — `adapters` decides the protocol, the engine keeps
/// sovereignty). Every adapter attempt reports through these three calls so
/// the loop is observable as data: one execution row per attempt.
pub trait FormLoopHost {
    /// A provider attempt is about to run: open its execution row
    /// (`attempt` is 1-based; a new RoleExecution row per attempt).
    fn attempt_started(&mut self, attempt: u32);
    /// The attempt was rejected by the FORM gate: close its row as failed
    /// with class `response_form` and record the FULL violation list — a
    /// loud event, never a silent retry (REQ-FG-02, REQ-FG-07).
    fn attempt_rejected(&mut self, attempt: u32, violations: &[String]);
    /// The attempt failed for a NON-form reason (provider/process/platform):
    /// close its row with the given failure class. The loop does NOT
    /// continue — form attempts are never spent on provider failures
    /// (REQ-FG-03).
    fn attempt_failed(&mut self, attempt: u32, class: &'static str, error: &str);
}

/// Terminal result of [`AgentRuntime::run_with_form_loop`].
#[derive(Debug)]
pub enum RunOutcome {
    /// A contract-valid reply; its attempt row stays open for the engine to
    /// finish (stamp / assembly / publish may still fail it honestly).
    Output(ExecOutput),
    /// Every allowed attempt was rejected on FORM; `violations` is the last
    /// attempt's FULL list — what the escalation question carries (OQ-FG-08).
    FormExhausted { attempts: u32, violations: Vec<String> },
    /// A non-form failure; its row was closed by the host. No attempt
    /// budget was consumed beyond the failing call (REQ-FG-03).
    Failed(AdapterError),
}

impl AgentRuntime {
    pub fn run(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        match req.provider.as_str() {
            "mock" => Self::mock(req),
            "claude" => Self::claude(req),
            "hermes" => Self::hermes(req),
            // Test/protocol fixtures for the form loop, in the same spirit as
            // `mock` (offline, deterministic): `mock-form-flaky` violates the
            // contract on its FIRST answer and fixes the FORM (never the
            // substance) on every re-reply — the synthetic-incident drive for
            // REQ-FG-02 AC1; `mock-form-broken` violates on every attempt,
            // driving exhaustion/escalation. Not seeded as agents anywhere.
            "mock-form-flaky" => Self::mock_form_flaky(req),
            "mock-form-broken" => Self::mock_form_broken(req),
            "mock-spec-noecho" => Self::mock_spec_no_echo(req),
            other => Err(AdapterError::UnknownProvider(other.to_string())),
        }
    }

    /// THE response-form correction loop (REQ-FG-02): run the role; if the
    /// reply is rejected on FORM, hand the SAME role its full violation list
    /// for a re-reply (fresh sessionless process — the repair prompt must
    /// carry the context, `ExecRequest::repair`), up to `max_repair_attempts`
    /// re-prompts on top of the initial call. While attempts remain nothing
    /// fails at all; on exhaustion the caller escalates (OQ-FG-08) or, with
    /// `max_repair_attempts = 0`, behaves exactly as before the loop existed
    /// (REQ-FG-04 AC2). A non-form error aborts immediately (REQ-FG-03).
    /// The loop publishes nothing: rejected bodies never become artifacts
    /// (REQ-FG-07 AC2).
    pub fn run_with_form_loop(req: &ExecRequest, max_repair_attempts: u32, host: &mut dyn FormLoopHost) -> RunOutcome {
        let total = 1 + max_repair_attempts;
        let mut req = req.clone();
        let mut violations: Vec<String> = Vec::new();
        for attempt in 1..=total {
            host.attempt_started(attempt);
            req.repair = if attempt == 1 { None } else { Some((attempt, violations.clone())) };
            match Self::run(&req) {
                Ok(out) => return RunOutcome::Output(out),
                Err(AdapterError::BadOutput { violations: v }) => {
                    host.attempt_rejected(attempt, &v);
                    violations = v;
                    if attempt == total {
                        return RunOutcome::FormExhausted { attempts: attempt, violations };
                    }
                }
                Err(e) => {
                    let (class, text) = (e.failure_class(), e.to_string());
                    host.attempt_failed(attempt, class, &text);
                    return RunOutcome::Failed(e);
                }
            }
        }
        unreachable!("the loop returns on every attempt")
    }

    // -----------------------------------------------------------------
    // claude: headless CLI call, text -> contract JSON
    // -----------------------------------------------------------------

    fn claude(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        let system = build_system_prompt(&req.role);
        let contract = req.role.output_contract.as_ref().map(|c| (c.name.as_str(), c.version));
        let mut user = build_user_prompt(&req.title, &req.business, &req.inputs, contract, req.contract_schema.as_deref());
        // Attempt N+1 of the form loop: original context (kept above by the
        // unchanged request fields) PLUS the full violation list of attempt
        // N-1... as data (REQ-FG-02 AC2). The sessionless process can only
        // repair what this prompt carries.
        if let Some((attempt, violations)) = &req.repair {
            user.push_str(&form_repair_section(*attempt, violations));
        }

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
        // OQ-FG-02 (ratified): an unextractable / unparseable reply IS the
        // form class — the adapter already distinguishes it from a provider
        // failure above — so the loop can repair it. Left as an Agent-class
        // error, 'agent reply contains no JSON object' would keep eating the
        // full role re-run, which is exactly this task's incident.
        let json = extract_json_object(text)
            .ok_or_else(|| AdapterError::BadOutput { violations: vec!["agent reply contains no JSON object".into()] })?;
        let parsed: Value = serde_json::from_str(&json)
            .map_err(|e| AdapterError::BadOutput { violations: vec![format!("extracted object is not valid JSON: {e}")] })?;
        validate_against_contract(&json, req.contract_schema.as_deref())?;
        Ok(ExecOutput { content: json.into_bytes(), summary: summarize(contract.map(|c| c.0), &parsed) })
    }

    // -----------------------------------------------------------------
    // hermes: headless one-shot CLI call, bare text -> contract JSON
    // -----------------------------------------------------------------

    /// Drive the Hermes CLI one-shot. Interface facts were checked against
    /// the live `hermes --help` (ratified requirements approval): `-z
    /// PROMPT` prints ONLY the final response text to stdout (no banner,
    /// no session line); `--usage-file PATH` writes the JSON usage report
    /// after a `-z` run — even when the run fails; `-m MODEL` /
    /// `--provider PROVIDER` / `-t TOOLSETS` scope one invocation. The
    /// envelope difference from `claude` (ASM-3) is the ONLY difference on
    /// the reply path: there is no `{"result": ...}` JSON envelope to
    /// unwrap, so contract validation starts directly at
    /// `extract_json_object` on the raw stdout text. Everything after that
    /// — extract, gate, form class, summary — is the shared claude path
    /// (provider-agnostic contract layer, R1). A non-zero exit is the
    /// provider-class failure (`Agent`), never a form rejection, so the
    /// loop spends no attempts on it (REQ-FG-03, R1 AC3).
    fn hermes(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        let mut cmd = std::process::Command::new("hermes");
        cmd.args(Self::hermes_argv(req)?);
        // R4 / REQ-WT-02 AC1: cwd is the platform workspace (the isolation
        // worktree for a writing role), identical mechanics to claude —
        // never an agent-chosen directory.
        if let Some(ws) = &req.workspace {
            cmd.current_dir(ws);
        }
        // REQ-WT-05 (R4): the run-artifact sink is platform-declared and
        // exported; the agent does not choose where its logs go.
        if let Some(ev) = &req.evidence {
            cmd.env("DEVAI_EVIDENCE_DIR", ev);
        }
        // Ratified OQ-5: no `--ignore-user-config` / `--safe-mode` by
        // default — the inference keys live in hermes' own user config, and
        // forcing isolation would break the R3 premise (keys outside the
        // store). An operator who wants a hermetised agent keeps that
        // choice in hermes config, out of platform argv.
        let out = cmd.output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(AdapterError::Agent(format!("`hermes` exited with {}: {}", out.status, err.trim())));
        }
        Self::hermes_text_to_output(req, &String::from_utf8_lossy(&out.stdout))
    }

    /// The hermes reply path, pure and offline-testable (R7): bare stdout
    /// text -> contract-shaped output, form failures carry the violation
    /// list exactly as the claude envelope path does.
    fn hermes_text_to_output(req: &ExecRequest, text: &str) -> Result<ExecOutput, AdapterError> {
        // OQ-FG-02 (ratified) applies unchanged: unextractable text IS the
        // form class — the same repairable rejection as on claude.
        let json = extract_json_object(text)
            .ok_or_else(|| AdapterError::BadOutput { violations: vec!["agent reply contains no JSON object".into()] })?;
        let parsed: Value = serde_json::from_str(&json)
            .map_err(|e| AdapterError::BadOutput { violations: vec![format!("extracted object is not valid JSON: {e}")] })?;
        validate_against_contract(&json, req.contract_schema.as_deref())?;
        let contract = req.role.output_contract.as_ref().map(|c| c.name.as_str());
        Ok(ExecOutput { content: json.into_bytes(), summary: summarize(contract, &parsed) })
    }

    // -----------------------------------------------------------------
    // hermes: pure argv/prompt builders (R7 AC: no live binary needed)
    // -----------------------------------------------------------------

    /// Glue the role framing into the ONE prompt string (`-z` takes a
    /// single prompt; there is no `--append-system-prompt` analog in the
    /// live usage — ratified OQ-3). System section first, then the task
    /// body built by the SAME `build_user_prompt` the claude path uses,
    /// repair section included (REQ-FG-02 AC2: the session-independent
    /// one-shot can only fix its form from what this text carries).
    fn hermes_prompt(req: &ExecRequest) -> String {
        let system = build_system_prompt(&req.role);
        let contract = req.role.output_contract.as_ref().map(|c| (c.name.as_str(), c.version));
        let mut user = build_user_prompt(&req.title, &req.business, &req.inputs, contract, req.contract_schema.as_deref());
        if let Some((attempt, violations)) = &req.repair {
            user.push_str(&form_repair_section(*attempt, violations));
        }
        if system.is_empty() {
            return user;
        }
        format!("=== ROLE (system instructions) ===\n{system}\n\n=== TASK ===\n{user}")
    }

    /// Pure argv builder for one `hermes -z` invocation. Flag sources are
    /// the ratified set: `-m` from `ExecRequest.model` (R3), `--provider`
    /// from `provider_override` only when it is a real override (OQ-4),
    /// `-t` from the capability table below (R2/OQ-2), `--usage-file`
    /// inside the platform evidence dir only when one exists (R5; ratified
    /// OQ-1: writing roles only — read-only roles keep `evidence = None`,
    /// extending evidence there is a separate task).
    fn hermes_argv(req: &ExecRequest) -> Result<Vec<String>, AdapterError> {
        let mut argv = vec!["-z".to_string(), Self::hermes_prompt(req)];
        let toolsets = hermes_toolsets_for(&req.role)?;
        // R2: empty `tools:` == run without `-t` (hermes keeps its own
        // configured posture; the operator's user config is not silently
        // rewritten by the platform — consistent with OQ-5).
        if !toolsets.is_empty() {
            argv.push("-t".into());
            argv.push(toolsets.join(","));
        }
        if !req.model.is_empty() {
            argv.push("-m".into());
            argv.push(req.model.clone());
        }
        if has_provider_override(&req.provider_override) {
            argv.push("--provider".into());
            argv.push(req.provider_override.clone());
        }
        if let Some(path) = hermes_usage_path(req) {
            argv.push("--usage-file".into());
            argv.push(path.display().to_string());
        }
        Ok(argv)
    }

}

/// File name of the collected usage report inside the evidence dir (R5).
const HERMES_USAGE_FILE: &str = "usage.json";

/// The usage path when this execution HAS a platform evidence dir;
/// `None` exactly when it does not (ratified OQ-1: read-only roles run
/// with `evidence = None`, so `--usage-file` is not passed to them).
pub fn hermes_usage_path(req: &ExecRequest) -> Option<std::path::PathBuf> {
    req.evidence.as_ref().map(|ev| ev.join(HERMES_USAGE_FILE))
}

/// R5 / ratified OQ-7: the usage report is EXPECTED only from a `hermes`
/// run inside a platform evidence dir. The engine asks THIS (the provider
/// vocabulary lives in the driver, the engine only writes the event —
/// sovereignty discipline) and records the ABSENCE as a loud fact event,
/// never silence: silence would defeat the very reason usage was
/// introduced — cost as an observable execution fact.
pub fn expects_usage_report(req: &ExecRequest) -> Option<std::path::PathBuf> {
    if req.provider == "hermes" {
        hermes_usage_path(req)
    } else {
        None
    }
}

/// Capability -> toolset names for hermes (ratified OQ-2 table): `file` =
/// read/write/patch/search files, `terminal` = command execution + process
/// management — names verified against the hermes toolset registry
/// (`~/.hermes/hermes-agent/toolsets.py`). The driver only ever spells
/// names from this const table, so a bogus `-t` value (`invalid_params` at
/// hermes runtime) cannot reach argv by construction. This is the DRIVER
/// translation layer, in the same spirit as `claude_tools_for` — WHICH
/// capabilities a role has stays config.
///
/// Unknown capabilities fail LOUDLY at argv assembly (OQ-2: an unknown
/// capability does not silently pass) — silently granting nothing could
/// let a role that believes it has tools run with none. `Platform` class:
/// the role config and the driver table are out of sync, a platform-side
/// assembly fact; never replayed to the role, never burns a form attempt
/// (REQ-FG-03). `api:*` capabilities are the DECLARED no-tool affordances
/// of the platform vocabulary (R2 AC: `api:project-knowledge`,
/// `api:artifacts` grant nothing on any CLI driver) — known, not a mapping
/// gap, hence not an error.
const HERMES_TOOLSET_FILE: &str = "file";
const HERMES_TOOLSET_TERMINAL: &str = "terminal";

fn hermes_toolsets_for(role: &RoleDefinition) -> Result<Vec<String>, AdapterError> {
    let mut out: Vec<String> = Vec::new();
    for cap in &role.tools {
        match cap.as_str() {
            "filesystem:read" | "filesystem:read-write" => out.push(HERMES_TOOLSET_FILE.into()),
            "git" => out.push(HERMES_TOOLSET_TERMINAL.into()),
            other if other.starts_with("shell:") => out.push(HERMES_TOOLSET_TERMINAL.into()),
            other if other.starts_with("api:") => {}
            other => {
                return Err(AdapterError::Platform(format!(
                    "role `{}` declares capability `{other}`, which has no known hermes toolset mapping: refusing to build argv (extend the driver table or fix the role config — no silent grant, no silent drop)",
                    role.slug
                )));
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Ratified OQ-4, reconciled with R6 AC: `--provider` is passed only for a
/// REAL override. Empty means absence; `local` is the neutral default
/// `devai agent add --runtime` writes when the operator named no LLM
/// provider — it is not a hermes provider and would make the R6 flow
/// (`agent add --provider hermes`, then `task run`) fail, so it reads as
/// absence too. A named provider (`--runtime anthropic|openrouter|...`)
/// is passed through verbatim.
fn has_provider_override(s: &str) -> bool {
    !s.is_empty() && s != "local"
}

impl AgentRuntime {
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
                // Input-section labels, echoed deterministically: this is the
                // ratified observation surface (OQ-F precedent) for "the role
                // actually RECEIVES the stored decisions section" — ratified
                // OQ-RAT-1 dogfood; the labels, never their contents.
                "notes": format!("mock implementation of {}; input sections: [{}]", one_line(&req.title), req.inputs.iter().map(|(l, _)| l.split(" (").next().unwrap_or(l)).collect::<Vec<_>>().join(", ")),
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
            // Seed/schema drift is a PLATFORM bug, not the fixture role's
            // form error — it must not burn loop re-prompts (REQ-FG-03).
            .map_err(|e| AdapterError::Platform(format!("mock output invalid (seed/schema drift): {e}")))?;
        Ok(ExecOutput { content: text, summary: summarize(Some(name), &json) })
    }

    /// Protocol fixture: first answer violates the stored contract, every
    /// re-reply (a request carrying `repair`) answers in the same shape as
    /// `mock` — a role that FIXES THE FORM, not the substance. Drives the
    /// synthetic incident of REQ-FG-02 AC1 offline and deterministically.
    fn mock_form_flaky(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        if req.repair.is_some() {
            return Self::mock(req);
        }
        Self::mock_form_broken(req)
    }

    /// Protocol fixture for REQ-FG-06 / ratified OQ-FG-07: the analyst
    /// answers in a fully LEGAL @2 shape but SKIPS the echo fields
    /// (`task_id`, `business_description`) — exactly the form @2 exists to
    /// allow, so the platform-side stamping is what fills them. Every other
    /// role behaves like plain `mock`.
    fn mock_spec_no_echo(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        if req.role.slug != "requirements-analyst" {
            return Self::mock(req);
        }
        let out = Self::mock(req)?;
        let mut body: Value = serde_json::from_slice(&out.content).map_err(|e| AdapterError::Agent(e.to_string()))?;
        let obj = body.as_object_mut().ok_or_else(|| AdapterError::Agent("mock spec reply is not an object".to_string()))?;
        obj.remove("task_id");
        obj.remove("business_description");
        let text = serde_json::to_vec_pretty(&body).map_err(|e| AdapterError::Agent(e.to_string()))?;
        // LEGAL under @2 precisely because the echo fields are optional and
        // declared (OQ-FG-03/07) — a schema regression would fail this here,
        // loudly, not silently at the stamp.
        validate_against_contract(&String::from_utf8_lossy(&text), req.contract_schema.as_deref())
            .map_err(|e| AdapterError::Platform(format!("no-echo fixture invalid under the stored schema: {e}")))?;
        Ok(ExecOutput { content: text, summary: out.summary })
    }

    /// Protocol fixture: violates the stored contract on EVERY attempt,
    /// whatever the repair context — drives loop exhaustion (REQ-FG-02 AC3,
    /// OQ-FG-08 escalation) offline.
    fn mock_form_broken(req: &ExecRequest) -> Result<ExecOutput, AdapterError> {
        // The historical incident shape: a plausible object missing exactly
        // one required key. Violations come from the SAME gate as real
        // replies — never a hand-written error here.
        let json = serde_json::json!({ "mock": "form-broken", "requirements": [] });
        let text = serde_json::to_vec_pretty(&json).map_err(|e| AdapterError::Agent(e.to_string()))?;
        validate_against_contract(&String::from_utf8_lossy(&text), req.contract_schema.as_deref())?;
        // Valid under no stored reply contract (no required keys satisfied);
        // if a test passes no schema at all, manufacture the form failure.
        Err(AdapterError::BadOutput { violations: vec!["mock-form-broken: no contract to fail".into()] })
    }
}

/// The loud repair section of attempt N+1 (REQ-FG-02 AC2): what failed, the
/// FULL violation list verbatim, and the standing null policy (OQ-FG-03
/// strict-write half). Carried in the prompt because the role process is
/// sessionless — the loop has no other channel to say "what to fix".
pub fn form_repair_section(attempt: u32, violations: &[String]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = write!(
        s,
        "\nOUTPUT REJECTED BY THE CONTRACT GATE (attempt {attempt}): your previous reply did not pass the stored reply-shape contract. This is a FORM problem, not a judgment of your content — keep the substance and every source-verified fact, fix the shape. Full violation list of the previous attempt:\n"
    );
    for v in violations {
        let _ = writeln!(s, "  - {v}");
    }
    let _ = write!(
        s,
        "Re-emit the COMPLETE JSON object satisfying the schema above: every required key present with its declared type; an optional field you cannot fill is OMITTED entirely — never written as null. Return only the JSON object.\n"
    );
    s
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
/// checks, with a json path on EACH violation (the full list, never only the
/// first — REQ-FG-01 AC2). Full JSON-Schema is still out of scope, but
/// garbage cannot silently enter the artifact store any more (defect R2 of
/// the dogfood task). `minItems` is what makes the QA rule "every criteria
/// entry cites concrete evidence" mechanically enforced, not just declared.
///
/// Null policy (OQ-FG-03, ratified — all three layers):
/// * schema: optional typed fields declare nullability (`["x","null"]`) —
///   the stored schema is the single truth for producer and validator;
/// * this READ half: an explicit null on an OPTIONAL key is accepted as
///   absence (lenient read); on a REQUIRED key it stays a violation —
///   precedent 2.11's `expected object, got null` is pinned by test;
/// * WRITE half (prompts/common.yaml + repair section): producers OMIT
///   absent fields, never write null.
pub fn validate_against_contract(content: &str, schema: Option<&str>) -> Result<(), AdapterError> {
    let violations = contract_violations(content, schema)?;
    if !violations.is_empty() {
        return Err(AdapterError::BadOutput { violations });
    }
    Ok(())
}

/// The form gate as DATA: every violation with its json path, whole.
/// `Err` is reserved for the PLATFORM half — a broken stored schema the role
/// can neither see nor fix (REQ-FG-03 AC2: never replayed to the role).
/// A reply that is not parseable JSON, or is JSON but not an object, is
/// FORM-class (ratified OQ-FG-02) and therefore arrives as a violation list.
pub fn contract_violations(content: &str, schema: Option<&str>) -> Result<Vec<String>, AdapterError> {
    let Some(schema) = schema else { return Ok(Vec::new()); };
    let value: Value = serde_json::from_str(content)
        .map_err(|e| AdapterError::BadOutput { violations: vec![format!("$: output is not valid JSON: {e}")] })?;
    if !value.is_object() {
        return Ok(vec![format!("$: output must be a JSON object, got {}", json_kind(&value))]);
    }
    let schema_v: Value = serde_json::from_str(schema)
        .map_err(|e| AdapterError::Platform(format!("stored contract schema is broken: {e}")))?;
    let mut errs = Vec::new();
    validate_node(&schema_v, &value, "$", true, &mut errs);
    Ok(errs)
}

/// Declared types of a schema node — a string or a nullability array
/// (`["string","null"]`, the schema-authority declaration of OQ-FG-03).
fn schema_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(|t| t.as_str()).collect(),
        _ => vec![],
    }
}

fn validate_node(schema: &Value, value: &Value, path: &str, required: bool, errs: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else { return; };
    let types = schema_types(schema);
    if types.is_empty() {
        return;
    }
    if value.is_null() {
        // Nullable declaration, or explicit null on an OPTIONAL key: absence
        // is honoured, nothing to check below (OQ-FG-03 lenient read). On a
        // REQUIRED key the type dispatch continues and reports the mismatch.
        if types.contains(&"null") || !required {
            return;
        }
    }
    let s = types.iter().copied().find(|t| *t != "null").unwrap_or("null");
    match s {
        "object" => {
            let Some(map) = value.as_object() else {
                errs.push(format!("{path}: expected object, got {}", json_kind(value)));
                return;
            };
            let req_keys: Vec<&str> = obj
                .get("required")
                .and_then(|r| r.as_array())
                .map(|a| a.iter().filter_map(|r| r.as_str()).collect())
                .unwrap_or_default();
            for k in &req_keys {
                if !map.contains_key(*k) {
                    errs.push(format!("{path}.{k}: missing required key"));
                }
            }
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                for (k, v) in map {
                    if let Some(sub) = props.get(k) {
                        // The child's required-ness decides its null policy:
                        // REQUIRED + null keeps failing the type check below.
                        validate_node(sub, v, &format!("{path}.{k}"), req_keys.iter().any(|r| r == k), errs);
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
                        validate_node(sub, it, &format!("{path}[{i}]"), true, errs);
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
            provider_override: "".into(),
            inputs: vec![],
            contract_schema: None,
            workspace: None,
            evidence: None,
            repair: None,
        }
    }

    /// Test host for the form loop: records every callback in order — the
    /// observation discipline of REQ-FG-07 (loud loop, no silent retries).
    struct LoopRecorder {
        log: Vec<String>,
    }
    impl LoopRecorder {
        fn new() -> Self {
            LoopRecorder { log: vec![] }
        }
    }
    impl FormLoopHost for LoopRecorder {
        fn attempt_started(&mut self, attempt: u32) {
            self.log.push(format!("started:{attempt}"));
        }
        fn attempt_rejected(&mut self, attempt: u32, violations: &[String]) {
            self.log.push(format!("rejected:{attempt}:{}", violations.join("|")));
        }
        fn attempt_failed(&mut self, attempt: u32, class: &'static str, error: &str) {
            self.log.push(format!("failed:{attempt}:{class}:{error}"));
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
        assert!(matches!(validate_against_contract(r#"{"summary":"x"}"#, Some(schema)), Err(AdapterError::BadOutput { .. })));
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

    /// REQ-FG-05 AC2 + ratified OQ-FG-03, pinned as REGRESSION behavior:
    /// the three-layer null policy. Optional + explicit null -> accepted as
    /// absence (lenient READ); required + explicit null -> the historical
    /// 2.11 violation stays (`expected object, got null`); a nullability
    /// declaration (`["x","null"]`) keeps TYPE checks for non-null values.
    #[test]
    fn null_policy_is_layered_absence_vs_strict_required() {
        let schema = r#"{
            "type": "object",
            "required": ["verdict", "evaluation"],
            "properties": {
                "verdict": { "type": ["string", "null"] },
                "summary": { "type": "string" },
                "evaluation": { "type": "object", "required": ["score"], "properties": { "score": { "type": "number" } } }
            }
        }"#;
        // PRESENT null on the OPTIONAL `summary` key = absence, gate passes.
        let lenient = r#"{"verdict":"ACCEPT","summary":null,"evaluation":{"score":1.0}}"#;
        assert!(validate_against_contract(lenient, Some(schema)).is_ok(), "explicit null on an optional field reads as absence");
        // PRESENT null on a REQUIRED object field keeps the 2.11 shape.
        let strict = r#"{"verdict":"ACCEPT","evaluation":null}"#;
        let e = validate_against_contract(strict, Some(schema)).unwrap_err().to_string();
        assert!(e.contains("$.evaluation: expected object, got null"), "precedent 2.11 pinned: {e}");
        // Nullability declaration still type-checks NON-null values.
        let wrong = r#"{"verdict":7,"evaluation":{"score":1.0}}"#;
        let e = validate_against_contract(wrong, Some(schema)).unwrap_err().to_string();
        assert!(e.contains("$.verdict: expected string, got number"), "{e}");
        // ...and accepts null where the schema declares it nullable.
        let nullable_ok = r#"{"verdict":null,"evaluation":{"score":1.0}}"#;
        assert!(validate_against_contract(nullable_ok, Some(schema)).is_ok());
    }

    /// REQ-FG-01 AC2: the gate keeps the FULL list with json paths — nothing
    /// is truncated to the first violation (it is what the role is handed).
    #[test]
    fn gate_returns_the_whole_violation_list_as_data() {
        let schema = r#"{"type":"object","required":["a","b"],"properties":{"a":{"type":"string"},"b":{"type":"string"},"c":{"type":"integer"}}}"#;
        let e = validate_against_contract(r#"{"b":7,"c":"x"}"#, Some(schema)).unwrap_err();
        let AdapterError::BadOutput { violations } = &e else { panic!("form class expected: {e}") };
        assert!(violations.contains(&"$.a: missing required key".to_string()), "{violations:?}");
        assert!(violations.iter().any(|v| v.starts_with("$.b: expected string")), "{violations:?}");
        assert!(violations.iter().any(|v| v.starts_with("$.c: expected integer")), "{violations:?}");
        assert_eq!(violations.len(), 3, "every violation, none lost");
    }

    /// REQ-FG-03: a broken STORED schema is a platform half — it never
    /// masquerades as a role form error the loop could repair.
    #[test]
    fn broken_stored_schema_is_platform_not_form() {
        let e = validate_against_contract(r#"{"a":1}"#, Some("{ not json")).unwrap_err();
        assert!(matches!(e, AdapterError::Platform(_)), "{e}");
    }

    /// OQ-FG-02 (ratified): unparseable output IS the form class — both in
    /// the raw-JSON half and via the extractable-object half of claude().
    #[test]
    fn unparseable_output_is_form_class() {
        let schema = r#"{"type":"object","required":["a"],"properties":{"a":{"type":"integer"}}}"#;
        let v = contract_violations("{ broken", Some(schema)).unwrap_err();
        assert!(matches!(v, AdapterError::BadOutput { .. }), "{v}");
        // A JSON scalar is FORM too (violations, not platform).
        let v = contract_violations("7", Some(schema)).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].starts_with("$: output must be a JSON object"), "{v:?}");
    }

    /// REQ-FG-02 AC1, offline and deterministic: one schema violation on the
    /// synthetic first answer is self-repaired by the SAME role with zero
    /// operator intervention — the loop re-presents the FULL list and the
    /// second attempt publishes.
    #[test]
    fn form_loop_repairs_a_single_violation_without_the_operator() {
        let mut r = req("mock-form-flaky", "review-report");
        r.contract_schema = Some(r#"{"type":"object","required":["verdict","summary","findings"],"properties":{"verdict":{"type":"string"},"summary":{"type":"string"},"findings":{"type":"array"}}}"#.into());
        let mut host = LoopRecorder::new();
        let out = AgentRuntime::run_with_form_loop(&r, 2, &mut host);
        assert!(matches!(out, RunOutcome::Output(_)), "role self-repaired: {out:?}" );
        assert_eq!(&host.log[0], "started:1");
        assert!(host.log[1].starts_with("rejected:1:$.business_description") || host.log[1].starts_with("rejected:1:$"), "rejection names the missing keys: {}", host.log[1]);
        assert_eq!(&host.log[2], "started:2");
        assert_eq!(host.log.len(), 3, "no provider call after success");
    }

    /// REQ-FG-02/04: exhaustion stops at `1 + max_repair_attempts` provider
    /// calls, keeping the LAST full violation list for escalation; with a
    /// budget of 0 the behavior is exactly today's — one call, then fail.
    #[test]
    fn form_loop_exhausts_at_the_configured_budget() {
        let mut r = req("mock-form-broken", "review-report");
        r.contract_schema = Some(r#"{"type":"object","required":["verdict"],"properties":{"verdict":{"type":"string"}}}"#.into());
        let mut host = LoopRecorder::new();
        match AgentRuntime::run_with_form_loop(&r, 2, &mut host) {
            RunOutcome::FormExhausted { attempts, violations } => {
                assert_eq!(attempts, 3, "1 initial + 2 repair re-prompts (OQ-FG-01 default)");
                assert!(violations.iter().any(|v| v.contains("missing required key")), "{violations:?}");
            }
            other => panic!("exhaustion expected, got {other:?}"),
        }
        assert!(host.log.iter().filter(|l| l.starts_with("started:")).count() == 3, "{:?}", host.log);

        // Zero budget: one call, then the form failure surfaces immediately.
        let mut host0 = LoopRecorder::new();
        match AgentRuntime::run_with_form_loop(&r, 0, &mut host0) {
            RunOutcome::FormExhausted { attempts, .. } => assert_eq!(attempts, 1, "0 repairs = today's behavior"),
            other => panic!("exhaustion expected: {other:?}"),
        }
        assert_eq!(host0.log.iter().filter(|l| l.starts_with("started:")).count(), 1, "no re-prompt at budget 0");
    }

    /// REQ-FG-03 AC1: provider/unknown failures never enter the loop —
    /// exactly one attempt, no repair burns, FAILED stays FAILED.
    #[test]
    fn provider_errors_never_enter_the_form_loop() {
        let r = req("telepathy", "review-report");
        let mut host = LoopRecorder::new();
        match AgentRuntime::run_with_form_loop(&r, 2, &mut host) {
            RunOutcome::Failed(AdapterError::UnknownProvider(_)) => {}
            other => panic!("provider abort expected: {other:?}"),
        }
        assert_eq!(host.log.len(), 2, "started + failed, no retries: {:?}", host.log);
        assert!(host.log[1].contains(":provider:"), "class is provider, not form: {}", host.log[1]);
    }

    /// REQ-FG-02 AC2: the attempt-N+1 prompt carries the FULL violation list
    /// of attempt N verbatim (the sessionless process has no other channel).
    #[test]
    fn repair_prompt_carries_the_full_violation_list() {
        let violations = vec!["$.business_description: missing required key".to_string(), "$.requirements[7].source: expected object, got null".to_string()];
        let s = form_repair_section(2, &violations);
        assert!(s.contains("$.business_description: missing required key"), "{s}");
        assert!(s.contains("$.requirements[7].source: expected object, got null"), "{s}");
        assert!(s.contains("attempt 2"), "{s}");
        assert!(s.contains("never written as null"), "write policy restated for the producer: {s}");
        assert!(s.contains("not a judgment of your content"), "form-vs-content split told to the role: {s}");
    }

    // ---- hermes driver: the R7 offline discipline — NO live hermes
    // binary is invoked by any test below; argv/prompt/reply-path are pure. ----

    #[test]
    fn hermes_argv_carries_prompt_toolsets_model_provider_usage() {
        let mut r = req("hermes", "implementation-report");
        r.role.tools = vec!["git".into(), "filesystem:read-write".into(), "shell:build".into(), "api:artifacts".into()];
        r.model = "anthropic/claude-sonnet-4.6".into();
        r.provider_override = "openrouter".into();
        r.evidence = Some("/evidence/task/exec".into());
        let argv = AgentRuntime::hermes_argv(&r).unwrap();
        assert_eq!(argv[0], "-z", "headless one-shot (R1)");
        assert!(argv[1].contains("TASK: Minimal process"), "user prompt rides -z");
        let pos = |f: &str| argv.iter().position(|a| a == f).expect(f);
        assert_eq!(argv[pos("-t") + 1], "file,terminal", "R2/OQ-2 table: filesystem->file, git/shell->terminal, api:*->nothing");
        assert_eq!(argv[pos("-m") + 1], "anthropic/claude-sonnet-4.6", "R3: -m only from req.model");
        assert_eq!(argv[pos("--provider") + 1], "openrouter", "OQ-4: override passed verbatim");
        assert_eq!(argv[pos("--usage-file") + 1], "/evidence/task/exec/usage.json", "R5: inside the PLATFORM evidence dir, never the tree");
    }

    #[test]
    fn hermes_argv_omits_absent_overrides_and_usage() {
        let mut r = req("hermes", "review-report");
        // Read-only shape (ratified OQ-1): evidence=None => no --usage-file.
        r.evidence = None;
        r.role.tools = vec![];
        let argv = AgentRuntime::hermes_argv(&r).unwrap();
        assert_eq!(argv.len(), 2, "-z + prompt only; empty tools => no -t (R2)");
        for flag in ["-t", "-m", "--provider", "--usage-file"] {
            assert!(!argv.iter().any(|a| a == flag), "absent sources emit no flag (R2/R3/R5): {flag}");
        }
        // The neutral `agent add` default is absence, not a provider (OQ-4 x R6).
        r.provider_override = "local".into();
        assert!(!AgentRuntime::hermes_argv(&r).unwrap().iter().any(|a| a == "--provider"));
    }

    #[test]
    fn unknown_capability_is_a_loud_error_not_a_silent_grant() {
        let mut r = req("hermes", "review-report");
        r.role.tools = vec!["telepathy:read".into()];
        let e = AgentRuntime::hermes_argv(&r).unwrap_err();
        assert!(matches!(e, AdapterError::Platform(_)), "loud at argv assembly (ratified OQ-2): {e}");
        assert!(e.to_string().contains("telepathy:read"), "names the offending capability: {e}");
        assert_eq!(e.failure_class(), devai_core::types::failure_class::PLATFORM, "never burns form attempts (REQ-FG-03)");
        // Declared no-tool capabilities and empty tools are known, not errors.
        r.role.tools = vec!["api:project-knowledge".into()];
        assert!(AgentRuntime::hermes_argv(&r).unwrap().iter().all(|a| a != "-t"));
    }

    /// R1/R7: the hermes reply path — BARE text (no `{"result":...}`
    /// envelope, ASM-3) — reaches the SAME contract gate as claude: bad
    /// text is the FORM class with a violation list, never a provider
    /// failure; valid text passes exactly as the envelope path would.
    #[test]
    fn hermes_bare_text_reaches_the_same_contract_gate() {
        let mut r = req("hermes", "review-report");
        r.contract_schema = Some(r#"{"type":"object","required":["verdict","summary"]}"#.into());
        let out = AgentRuntime::hermes_text_to_output(&r, "prose before {\"verdict\":\"APPROVED\",\"summary\":\"ok\"} prose after")
            .expect("extract_json_object rides directly on raw stdout text");
        assert_eq!(String::from_utf8(out.content).unwrap(), r#"{"verdict":"APPROVED","summary":"ok"}"#);
        // Non-JSON text is a form rejection with the violation LIST (AC: form
        // class, not provider failure — the loop may repair it).
        let e = AgentRuntime::hermes_text_to_output(&r, "I cannot produce an object").unwrap_err();
        assert!(matches!(&e, AdapterError::BadOutput { violations } if !violations.is_empty()), "form class: {e}");
        assert_eq!(e.failure_class(), devai_core::types::failure_class::RESPONSE_FORM);
        // A missing required key yields the SAME violation string the claude
        // path produces — the gate is provider-agnostic (R1 constraint).
        let e = AgentRuntime::hermes_text_to_output(&r, r#"{"summary":"done"}"#).unwrap_err();
        assert!(matches!(&e, AdapterError::BadOutput { violations } if violations.contains(&"$.verdict: missing required key".to_string())), "{e}");
    }

    /// Ratified OQ-3: one glued prompt — system section before the task
    /// body — carrying the same repair data (REQ-FG-02 AC2), pure and
    /// offline-testable.
    #[test]
    fn hermes_prompt_glues_system_task_and_repair_sections() {
        let mut r = req("hermes", "review-report");
        r.repair = Some((2, vec!["$.verdict: missing required key".to_string()]));
        let p = AgentRuntime::hermes_prompt(&r);
        assert!(p.contains("=== ROLE (system instructions) ==="), "system section marker (OQ-3): {p}");
        assert!(p.contains("Devai `x` agent"), "build_system_prompt content survives the glue");
        assert!(p.contains("=== TASK ==="), "body section marker follows");
        assert!(p.find("ROLE (system").unwrap() < p.find("=== TASK ===").unwrap(), "system first, body after");
        assert!(p.contains("OUTPUT REJECTED BY THE CONTRACT GATE (attempt 2)"), "repair section rides the same single prompt (R1 AC4)");
        assert!(p.contains("$.verdict: missing required key"), "full violation list verbatim");
    }

    #[test]
    fn usage_fact_is_expected_only_for_hermes_with_evidence() {
        let mut r = req("hermes", "review-report");
        assert!(expects_usage_report(&r).is_none(), "no evidence dir => nothing to expect (OQ-1)");
        r.evidence = Some("/tmp/ev".into());
        assert_eq!(expects_usage_report(&r).unwrap().to_str().unwrap(), "/tmp/ev/usage.json");
        let c = req("claude", "review-report");
        assert!(expects_usage_report(&c).is_none(), "the claude driver has no usage analogue (motivation)");
    }

    /// R6: after a third live driver exists, the UnknownProvider text names
    /// the real drivers (it said `mock` or `claude` while that was truth).
    #[test]
    fn unknown_provider_error_names_the_real_drivers() {
        let e = AgentRuntime::run(&req("telepathy", "review-report")).unwrap_err();
        let s = e.to_string();
        for d in ["mock", "claude", "hermes"] {
            assert!(s.contains(d), "text lists the actual driver `{d}`: {s}");
        }
    }
}
