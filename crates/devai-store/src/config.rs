//! Filesystem-backed workflow configuration (`config/` in the devai home).
//!
//! Roles, basic acceptance checks and common prompt rules are DECLARATIVE
//! data, not code: editing the YAML and re-running `devai init` bumps the
//! role version, and the next execution picks it up. The engine itself keeps
//! only the sovereignty skeleton (state machine) in code — an agent cannot
//! rewrite the workflow through config, but a human tunes the criteria.
//!
//! Layout:
//! ```text
//! config/
//!   prompts/common.yaml        # base role rules
//!   criteria/review.yaml       # basic spec-acceptance checks (reviewer)
//!   evaluation/quality.yaml    # assessment profile (QA): gates, checks, scoring
//!   roles/<slug>.yaml          # role definitions
//! ```

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use devai_core::types::{
    ContractRef, RoleDefinition, Timestamp, Verdict, event_kind, json_field, new_id,
};
use serde::{Deserialize, Serialize};

use crate::{Store, StoreError, StoreResult};

// Embedded factory defaults — written into the home config on first init.
const DEFAULT_FILES: &[(&str, &str)] = &[
    (
        "prompts/common.yaml",
        include_str!("../../../config/prompts/common.yaml"),
    ),
    (
        "criteria/review.yaml",
        include_str!("../../../config/criteria/review.yaml"),
    ),
    (
        "evaluation/quality.yaml",
        include_str!("../../../config/evaluation/quality.yaml"),
    ),
    (
        "roles/project-intelligence.yaml",
        include_str!("../../../config/roles/project-intelligence.yaml"),
    ),
    (
        "roles/requirements-analyst.yaml",
        include_str!("../../../config/roles/requirements-analyst.yaml"),
    ),
    (
        "roles/requirements-reviewer.yaml",
        include_str!("../../../config/roles/requirements-reviewer.yaml"),
    ),
    (
        "roles/implementation-agent.yaml",
        include_str!("../../../config/roles/implementation-agent.yaml"),
    ),
    (
        "roles/quality-auditor.yaml",
        include_str!("../../../config/roles/quality-auditor.yaml"),
    ),
    (
        "roles/quality-aggregator.yaml",
        include_str!("../../../config/roles/quality-aggregator.yaml"),
    ),
    (
        "workflow/response-form-loop.yaml",
        include_str!("../../../config/workflow/response-form-loop.yaml"),
    ),
    (
        "workflow/rework-context.yaml",
        include_str!("../../../config/workflow/rework-context.yaml"),
    ),
];

fn cfg_err(msg: impl Into<String>) -> StoreError {
    StoreError::Config(msg.into())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Check {
    pub id: String,
    pub text: String,
    #[serde(default = "default_severity")]
    pub severity: String,
}

fn default_severity() -> String {
    "major".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Criteria {
    pub title: String,
    /// Role slug these checks are compiled into.
    pub applies_to: String,
    pub checks: Vec<Check>,
    pub verdict_rule: String,
}

// ---------------------------------------------------------------------------
// Evaluation profile (docs/evaluation-schema.md v0.2) — declarative QA rules.
// ---------------------------------------------------------------------------

fn yes() -> String {
    "required".into()
}
fn either() -> String {
    "either".into()
}

/// Structured probe call. `pass_exit` whitelists exit codes that count as
/// PASS (empty == only 0). Shell-level errors (126/127: not runnable/missing
/// binary) are BLOCKED, not FAIL — an unperformed check never pretends to
/// pass. Any other code is FAIL.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Probe {
    pub cmd: String,
    #[serde(default)]
    pub pass_exit: Vec<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GateDef {
    pub id: String,
    #[serde(default)]
    pub desc: String,
    pub probes: Vec<Probe>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ErrorTypeDef {
    pub desc: String,
    pub severity: String,
    #[serde(default)]
    pub default_fix_hint: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CheckDef {
    pub id: String,
    pub statement: String,
    pub detects: Vec<String>,
    /// mechanical | rubric | hybrid
    pub verify: String,
    #[serde(default)]
    pub probes: Vec<Probe>,
    #[serde(default)]
    pub anchors: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub evidence: String,
    /// human | agent | either | tool
    #[serde(default = "either")]
    pub reviewer: String,
    #[serde(default)]
    pub suppression: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppliesTo {
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EvaluationDef {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub applies_to: Option<AppliesTo>,
    pub checks: Vec<CheckDef>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BlockedPolicy {
    pub max_blocked_weight: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Scoring {
    #[serde(default = "per_check")]
    pub aggregation: String,
    pub theta: f64,
    /// severity -> minimal score; floors are DERIVED per check from the max
    /// severity of its `detects` — never declared on a check (V8).
    pub floors: BTreeMap<String, Option<f64>>,
    pub blocked_policy: BlockedPolicy,
    pub weights: BTreeMap<String, f64>,
}

fn per_check() -> String {
    "per-check".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Profile {
    pub version: u32,
    pub project: String,
    /// Profile identity for provenance (`name@version` recorded in every
    /// workspace revision). Optional in the YAML: when absent the loader
    /// fills it from the config file stem (`evaluation/quality.yaml` →
    /// `quality`), so an existing profile file needs no edit.
    #[serde(default)]
    pub name: Option<String>,
    /// devai extension binding the grammar to role compilation.
    #[serde(default)]
    pub applies_to_roles: Vec<String>,
    pub error_types: BTreeMap<String, ErrorTypeDef>,
    pub gates: Vec<GateDef>,
    pub evaluations: Vec<EvaluationDef>,
    pub scoring: Scoring,
}

const SEVERITIES: [&str; 4] = ["catastrophic", "major", "minor", "cosmetic"];
const VERIFY_KINDS: [&str; 3] = ["mechanical", "rubric", "hybrid"];
const REVIEWERS: [&str; 4] = ["human", "agent", "either", "tool"];

impl Profile {
    pub fn checks(&self) -> Vec<&CheckDef> {
        self.evaluations
            .iter()
            .flat_map(|e| e.checks.iter())
            .collect()
    }

    /// Provenance identity `name@version` recorded in every workspace
    /// revision. Falls back to `project` when no name is declared anywhere.
    pub fn identity(&self) -> String {
        format!(
            "{}@{}",
            self.name.as_deref().unwrap_or(&self.project),
            self.version
        )
    }

    /// Highest severity among the check's detectable error types (V8 source).
    /// SEVERITIES is ordered most-severe first, so a lower index wins.
    pub fn worst_severity(&self, c: &CheckDef) -> Option<&str> {
        let rank = |s: &str| SEVERITIES.iter().position(|x| *x == s);
        let mut best: Option<&str> = None;
        for t in &c.detects {
            let Some(sev) = self.error_types.get(t).map(|e| e.severity.as_str()) else {
                continue;
            };
            let replace = match best {
                None => true,
                Some(b) => match (rank(sev), rank(b)) {
                    (Some(r), Some(rb)) => r < rb,
                    _ => false,
                },
            };
            if replace {
                best = Some(sev);
            }
        }
        best
    }

    /// Floor for a check: min score derived from max severity of detects.
    pub fn floor_for(&self, c: &CheckDef) -> Option<f64> {
        self.worst_severity(c)
            .and_then(|sev| self.scoring.floors.get(sev).and_then(|v| *v))
    }

    pub fn weight_of(&self, id: &str) -> f64 {
        self.scoring.weights.get(id).copied().unwrap_or(0.0)
    }

    /// May the AGENT score this check itself (rubric/hybrid, not human/tool)?
    pub fn agent_scoreable(&self, c: &CheckDef) -> bool {
        c.verify != "mechanical" && matches!(c.reviewer.as_str(), "agent" | "either")
    }

    /// Grammar invariants V1-V8 validated at load time, before any execution.
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0 < self.scoring.theta && self.scoring.theta <= 1.0) {
            return Err(format!("theta {} out of (0, 1]", self.scoring.theta));
        }
        let cap = self.scoring.blocked_policy.max_blocked_weight;
        if !(0.0 <= cap && cap <= 1.0) {
            return Err(format!("max_blocked_weight {cap} out of [0, 1]"));
        }
        for (t, e) in &self.error_types {
            if !SEVERITIES.contains(&e.severity.as_str()) {
                return Err(format!(
                    "error_type `{t}`: severity `{}` not in {SEVERITIES:?}",
                    e.severity
                ));
            }
        }
        for s in self.scoring.floors.keys() {
            if !SEVERITIES.contains(&s.as_str()) {
                return Err(format!("floors: unknown severity `{s}`"));
            }
        }
        let mut ids: std::collections::HashSet<&str> = HashSet::new();
        for g in &self.gates {
            // V6: gates are binary by construction (GateDef has no anchors field).
            if g.probes.is_empty() {
                return Err(format!("gate `{}`: no probe", g.id));
            }
            if !ids.insert(g.id.as_str()) {
                return Err(format!("duplicate id `{}` (V2)", g.id));
            }
        }
        let mut n_checks = 0usize;
        let mut sum_w = 0.0f64;
        let mut weighted_ids: Vec<String> = Vec::new();
        for c in self.checks() {
            n_checks += 1;
            if !ids.insert(c.id.as_str()) {
                return Err(format!("duplicate id `{}` (V2)", c.id));
            }
            if !VERIFY_KINDS.contains(&c.verify.as_str()) {
                return Err(format!("check `{}`: verify `{}` invalid", c.id, c.verify));
            }
            if !REVIEWERS.contains(&c.reviewer.as_str()) {
                return Err(format!(
                    "check `{}`: reviewer `{}` invalid",
                    c.id, c.reviewer
                ));
            }
            // V1: every detected error type must be declared.
            for t in &c.detects {
                if !self.error_types.contains_key(t) {
                    return Err(format!(
                        "check `{}` detects undeclared error type `{t}` (V1)",
                        c.id
                    ));
                }
            }
            // V3: mechanical/hybrid need at least one probe.
            if matches!(c.verify.as_str(), "mechanical" | "hybrid") && c.probes.is_empty() {
                return Err(format!(
                    "check `{}` is {} without probe (V3)",
                    c.id, c.verify
                ));
            }
            // V4: rubric/hybrid need >=3 anchors incl. 1.0 and 0.0.
            if matches!(c.verify.as_str(), "rubric" | "hybrid") {
                if c.anchors.len() < 3
                    || !c.anchors.contains_key("1.0")
                    || !c.anchors.contains_key("0.0")
                {
                    return Err(format!(
                        "check `{}`: anchors must have >=3 gradations incl. \"1.0\" and \"0.0\" (V4)",
                        c.id
                    ));
                }
            }
            // V7: weights must cover exactly the check set and sum to 1.0.
            match self.scoring.weights.get(&c.id) {
                None => return Err(format!("check `{}` has no weight (V7)", c.id)),
                Some(w) => {
                    sum_w += w;
                    weighted_ids.push(c.id.clone());
                }
            }
        }
        for w in self.scoring.weights.keys() {
            if !weighted_ids.contains(w) {
                return Err(format!("weight for unknown check `{w}` (V7)"));
            }
        }
        if (sum_w - 1.0).abs() > 1e-9 {
            return Err(format!("weights sum to {sum_w}, must be 1.0 (V7)"));
        }
        let _ = n_checks;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoleFile {
    pub slug: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: u32,
    pub purpose: String,
    pub instructions: String,
    pub done_criteria: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub input_contract: Option<String>,
    #[serde(default)]
    pub output_contract: Option<String>,
}

fn default_version() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Common {
    #[serde(default)]
    pub title: String,
    pub rules: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub roles: Vec<RoleFile>,
    pub criteria: Vec<Criteria>,
    pub common: Common,
    /// Assessment profiles (declarative QA rules) from `evaluation/*.yaml`.
    pub profiles: Vec<Profile>,
}

impl Config {
    fn criteria_for(&self, slug: &str) -> Option<&Criteria> {
        self.criteria.iter().find(|c| c.applies_to == slug)
    }

    pub fn profile_for(&self, slug: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.applies_to_roles.iter().any(|r| r == slug))
    }

    /// Compile an applied profile into the role's checklist text: the agent
    /// sees ONLY the checks it may score, with anchors, plus the machine
    /// budget so its reply is shaped for platform assembly.
    fn compile_profile(profile: &Profile, ins: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            ins,
            "\n\nEvaluation profile `{}` v{}: the PLATFORM runs gates and mechanical probes, computes S, floors and the verdict — you never emit a verdict. Your reply is an evaluation-assessment JSON: `scores` ({{check-id: number}} for the rubric/hybrid checks below), `findings` ([{{check, text, evidence[]}}] — text = what + file:line + what it leads to; `evidence` is a NON-EMPTY array of concrete citations: test output, artifact id, build log), `gaps` ([{{check, reason}}] when you could not verify something honestly — an unperformed check is never a fake score). The platform assembles the quality-report@4 (bound to the workspace revision captured at execution start), and the contract gate rejects any criteria entry whose evidence array is missing or empty — no criterion is ever recorded as a pass without evidence.",
            profile.project, profile.version
        );
        let mut human_only: Vec<&str> = Vec::new();
        for c in profile.checks() {
            if c.verify == "mechanical" {
                continue;
            }
            if c.reviewer == "human" {
                human_only.push(c.id.as_str());
                continue;
            }
            if !profile.agent_scoreable(c) {
                continue;
            }
            let _ = writeln!(
                ins,
                "\n[{}] ({}; detects {}; weight {})",
                c.id,
                c.verify,
                c.detects.join(", "),
                profile.weight_of(&c.id)
            );
            let _ = writeln!(ins, "  statement: {}", c.statement.trim());
            if !c.anchors.is_empty() {
                // Best score first — the reviewer reads the ladder top-down.
                let mut anchors: Vec<(&String, &String)> = c.anchors.iter().collect();
                anchors.sort_by(|a, b| {
                    let (Ok(x), Ok(y)) = (a.0.parse::<f64>(), b.0.parse::<f64>()) else {
                        return b.0.cmp(a.0);
                    };
                    y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal)
                });
                let rendered: Vec<String> = anchors
                    .into_iter()
                    .map(|(k, v)| format!("{k} — {}", v.trim()))
                    .collect();
                let _ = writeln!(ins, "  anchors: {}", rendered.join(" | "));
            }
            if c.evidence == "required" {
                let _ = writeln!(
                    ins,
                    "  evidence: REQUIRED — every criteria entry must cite at least one concrete evidence item (test output, artifact id, build log). A score < 1.0 without evidence is discarded (proc/evidence-completeness); a PASS backed only by your judgment is contract-invalid (quality-report@4: minItems 1 per check)."
                );
            }
        }
        if !human_only.is_empty() {
            let _ = writeln!(
                ins,
                "\nHuman-signed checks (do NOT score them — leave them out of `scores`; the platform marks them pending and the verdict stays not-ready until a human accepts or returns): {}",
                human_only.join(", ")
            );
        }
        let _ = writeln!(
            ins,
            "Scoring budget (platform-side, for honesty): threshold theta {}, floors {}.",
            profile.scoring.theta,
            {
                let f: Vec<String> = profile
                    .scoring
                    .floors
                    .iter()
                    .map(|(s, v)| {
                        format!(
                            "{s}: {}",
                            v.map(|x| x.to_string()).unwrap_or_else(|| "none".into())
                        )
                    })
                    .collect();
                f.join(", ")
            }
        );
    }

    /// Compile a role file into a stored RoleDefinition: base instructions
    /// plus the common rules and, when defined, the basic-checks checklist
    /// or the evaluation profile (a profile supersedes flat criteria).
    /// A malformed `contract:` ref is a LOUD config error (QA return7
    /// п.(г)): the previous `.ok().flatten()` dropped a typo'd contract
    /// silently — the role then ran uncontracted and the output gate no
    /// longer validated it, turning a config mistake into a security hole.
    pub fn build_role(&self, rf: &RoleFile, now: Timestamp) -> StoreResult<RoleDefinition> {
        let mut ins = rf.instructions.trim().to_string();
        if !self.common.rules.is_empty() {
            ins.push_str("\n\nWorkflow rules:\n");
            for r in &self.common.rules {
                ins.push_str(&format!("- {r}\n"));
            }
            ins = ins.trim_end().to_string();
        }
        if let Some(p) = self.profile_for(&rf.slug) {
            Self::compile_profile(p, &mut ins);
        } else if let Some(c) = self.criteria_for(&rf.slug) {
            ins.push_str(&format!("\n\nBasic acceptance checks — {}:\n", c.title));
            for ch in &c.checks {
                ins.push_str(&format!("[{}|{}] {}\n", ch.id, ch.severity, ch.text));
            }
            ins.push_str(&format!("\nVerdict rule: {}", c.verdict_rule.trim()));
        }
        Ok(RoleDefinition {
            id: new_id(),
            slug: rf.slug.clone(),
            version: rf.version.max(1),
            name: rf.name.clone(),
            purpose: rf.purpose.clone(),
            instructions: ins,
            tools: rf.tools.clone(),
            input_contract: parse_contract_ref(rf.input_contract.as_deref())?,
            output_contract: parse_contract_ref(rf.output_contract.as_deref())?,
            done_criteria: rf.done_criteria.clone(),
            created_at: now,
        })
    }
}

fn parse_contract_ref(s: Option<&str>) -> StoreResult<Option<ContractRef>> {
    match s {
        None | Some("") => Ok(None),
        Some(v) => Some(ContractRef::parse(v).map_err(|e| cfg_err(e.to_string()))).transpose(),
    }
}

/// Write missing factory config files into `dir`, file by file. Files that
/// already exist are NEVER clobbered (a user edit survives `devai init`),
/// and a factory file added in a new devai version appears in old homes on
/// the next init — per-file materialisation, not whole-tree.
pub fn materialize_if_absent(dir: &Path) -> StoreResult<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (rel, text) in DEFAULT_FILES {
        let path = dir.join(rel);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| cfg_err(format!("config path `{rel}` has no parent")))?,
        )?;
        if path.exists() {
            continue;
        }
        fs::write(&path, text)?;
        written.push(path);
    }
    Ok(written)
}

/// Load and validate every assessment profile in `evaluation/*.yaml`.
/// Grammar errors (V1-V8) are rejected HERE, before any execution runs.
pub fn load_profiles(dir: &Path) -> StoreResult<Vec<Profile>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut profiles = Vec::new();
    for p in &paths {
        let mut profile: Profile = parse_yaml(p)?;
        // Identity fallback: the file stem is the profile name unless the
        // YAML declares one explicitly.
        if profile.name.is_none() {
            profile.name = p.file_stem().map(|s| s.to_string_lossy().into_owned());
        }
        profile
            .validate()
            .map_err(|e| cfg_err(format!("{}: {e}", p.display())))?;
        profiles.push(profile);
    }
    Ok(profiles)
}

// ---------------------------------------------------------------------------
// Response-form correction loop (REQ-FG-04) — platform protocol data.
// ---------------------------------------------------------------------------

/// Declarative budget of the form loop: how many re-prompts the platform may
/// hand the SAME role after a reply-shape rejection before it parks the task
/// on a blocking question (OQ-FG-01/08, ratified). Lives in
/// `config/workflow/response-form-loop.yaml` — protocol of the platform, not
/// a role property; `0` turns the loop off (= pre-feature behavior).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ResponseFormPolicy {
    #[serde(default = "default_max_repair_attempts")]
    pub max_repair_attempts: u32,
}

fn default_max_repair_attempts() -> u32 {
    2
}

impl Default for ResponseFormPolicy {
    /// The factory bytes are the fallback: a home without the file behaves
    /// exactly like one that has it at defaults (same idiom as the embedded
    /// workflow config materialisation — never a second copy of the policy).
    fn default() -> Self {
        ResponseFormPolicy {
            max_repair_attempts: default_max_repair_attempts(),
        }
    }
}

/// Merge-scan `workflow/*.yaml` for the `response_form:` key. Other files in
/// the directory (e.g. `rework-context.yaml`, a top-level list) are left
/// alone: only a mapping that declares `response_form` participates.
/// A malformed declaration is a LOUD config error — a silently ignored typo
/// in a budget would quietly resurrect the old behavior.
pub fn load_response_form_policy(dir: &Path) -> StoreResult<ResponseFormPolicy> {
    let mut policy = ResponseFormPolicy::default();
    if !dir.is_dir() {
        return Ok(policy);
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    for p in &paths {
        let text =
            fs::read_to_string(p).map_err(|e| cfg_err(format!("read {}: {e}", p.display())))?;
        let doc: serde_yaml::Value = serde_yaml::from_str(&text)
            .map_err(|e| cfg_err(format!("parse {}: {e}", p.display())))?;
        if let Some(section) = doc.get("response_form") {
            // The loud message names the field and its meaning: a typo in a
            // budget must tell the operator exactly what to fix.
            policy = serde_yaml::from_value::<ResponseFormPolicy>(section.clone()).map_err(|e| {
                cfg_err(format!(
                    "{}: response_form.max_repair_attempts must be a non-negative integer (re-prompts on top of the first call; 0 disables the loop): {e}",
                    p.display()
                ))
            })?;
        }
    }
    Ok(policy)
}

// ---------------------------------------------------------------------------
// REWORK CONTEXT delivery policy (R-001..R-007) — platform workflow data.
// ---------------------------------------------------------------------------

fn default_verdict_field() -> String {
    "verdict".into()
}

/// One recorded-decision-event entry feeding the next execution of a role.
/// Every selector here is human-authored text and is validated against the
/// platform's own vocabularies at LOAD time (`event_kind::ALL`,
/// `json_field::ALL`, the `Verdict` vocabulary): a typo is an explicit config
/// error naming the file and field — never a delivery that silently matches
/// nothing or reads nothing (2026-11-02 QA return, block 2).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReworkTrigger {
    /// The `workflow_events.kind` that feeds delivery for this role.
    pub event: String,
    /// Payload field carrying the rejection text exactly as persisted —
    /// delivery reads, never rewrites.
    pub rationale_field: String,
    /// Payload field carrying the decision verdict for `only_if_verdict_not`.
    #[serde(default = "default_verdict_field")]
    pub verdict_field: String,
    /// Verdict filter: the event counts only when `payload[verdict_field]`
    /// differs from this stored value.
    #[serde(default)]
    pub only_if_verdict_not: Option<String>,
    /// Additionally quote the failing / sub-threshold items of the CURRENT
    /// quality report as data (the QA→REWORK flow only).
    #[serde(default)]
    pub attach_findings: bool,
}

/// One row of the delivery table: which triggers feed which role.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReworkPolicy {
    pub role: String,
    pub triggers: Vec<ReworkTrigger>,
}

impl ReworkPolicy {
    /// Grammar of the human-authored table, validated before any execution
    /// can depend on it.
    pub fn validate(&self) -> Result<(), String> {
        if self.role.trim().is_empty() {
            return Err("policy has an empty `role` slug".into());
        }
        if self.triggers.is_empty() {
            return Err(format!(
                "role `{}`: `triggers` is empty — a policy that delivers nothing is a typo, not a feature (delete the row instead)",
                self.role
            ));
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for t in &self.triggers {
            if !event_kind::ALL.contains(&t.event.as_str()) {
                return Err(format!(
                    "role `{}`: event `{}` is not a workflow event kind the platform records — a kind that is never written matches nothing (a new persisted kind MUST be added to event_kind::ALL)",
                    self.role, t.event
                ));
            }
            if !seen.insert(t.event.as_str()) {
                return Err(format!(
                    "role `{}`: duplicate trigger event `{}`",
                    self.role, t.event
                ));
            }
            for (field, value) in [
                ("rationale_field", &t.rationale_field),
                ("verdict_field", &t.verdict_field),
            ] {
                if !json_field::ALL.contains(&value.as_str()) {
                    return Err(format!(
                        "role `{}`: {field} `{value}` is not a payload field the platform writes — delivery would silently read NOTHING",
                        self.role
                    ));
                }
            }
            if let Some(excluded) = &t.only_if_verdict_not {
                // The filter compares against the STORED text, so the literal
                // must be the canonical spelling: `Verdict::parse` is
                // case-insensitive, the payload comparison is not, and a
                // value that parses but never equals the stored one silently
                // disables the filter.
                let parsed = Verdict::parse(excluded).map_err(|_| {
                    format!(
                        "role `{}`: only_if_verdict_not `{excluded}` is not a Verdict ({}) — the filter cannot be evaluated",
                        self.role,
                        Verdict::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(" | ")
                    )
                })?;
                if excluded != parsed.as_str() {
                    return Err(format!(
                        "role `{}`: only_if_verdict_not `{excluded}` names a real verdict but is not the canonical stored spelling `{}`; the filter compares stored payload text exactly",
                        self.role,
                        parsed.as_str()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Merge-scan `workflow/*.yaml` for REWORK-context policy files: a file whose
/// top level is a SEQUENCE of `{role, triggers}` rows participates; a
/// top-level MAPPING (e.g. `response-form-loop.yaml`) is left to its own
/// loader. Cross-file duplicate roles are rejected naming BOTH files — two
/// rows for one role would make delivery order depend on directory listings.
/// Absent directory is not an error (the caller falls back to factory bytes);
/// a present-but-malformed file IS a loud error.
pub fn load_rework_policies(dir: &Path) -> StoreResult<Vec<ReworkPolicy>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut by_role: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut policies = Vec::new();
    for p in &paths {
        let text =
            fs::read_to_string(p).map_err(|e| cfg_err(format!("read {}: {e}", p.display())))?;
        let doc: serde_yaml::Value = serde_yaml::from_str(&text)
            .map_err(|e| cfg_err(format!("parse {}: {e}", p.display())))?;
        if doc.is_mapping() {
            continue;
        }
        let file_policies: Vec<ReworkPolicy> = serde_yaml::from_value(doc).map_err(|e| {
            cfg_err(format!(
                "{}: a REWORK-context policy file must be a list of `{{role, triggers}}` rows: {e}",
                p.display()
            ))
        })?;
        for pol in file_policies {
            pol.validate()
                .map_err(|e| cfg_err(format!("{}: {e}", p.display())))?;
            if let Some(prev) = by_role.get(&pol.role) {
                return Err(cfg_err(format!(
                    "{}: role `{}` is already declared in {} — a role has exactly one rework policy row",
                    p.display(),
                    pol.role,
                    prev.display()
                )));
            }
            by_role.insert(pol.role.clone(), p.clone());
            policies.push(pol);
        }
    }
    Ok(policies)
}

/// The factory table read from the embedded YAML bytes — the fallback when a
/// home has no `workflow/` directory or file, so the delivery map lives in
/// exactly one place (this YAML), never as a second copy in code (R-007).
pub fn factory_rework_policies() -> Vec<ReworkPolicy> {
    let (_, text) = DEFAULT_FILES
        .iter()
        .find(|(rel, _)| *rel == "workflow/rework-context.yaml")
        .expect("factory rework-context.yaml is embedded in DEFAULT_FILES");
    let policies: Vec<ReworkPolicy> =
        serde_yaml::from_str(text).expect("factory rework-context.yaml parses");
    for p in &policies {
        p.validate().expect("factory rework-context.yaml validates");
    }
    policies
}

fn parse_yaml<T: DeserializeOwnedPlus>(path: &Path) -> StoreResult<T> {
    let text =
        fs::read_to_string(path).map_err(|e| cfg_err(format!("read {}: {e}", path.display())))?;
    serde_yaml::from_str(&text).map_err(|e| cfg_err(format!("parse {}: {e}", path.display())))
}

// Marker so the concrete types above (all Deserialize) can be used in parse_yaml.
pub trait DeserializeOwnedPlus: serde::de::DeserializeOwned {}
impl<T: serde::de::DeserializeOwned> DeserializeOwnedPlus for T {}

/// Load the whole config tree from disk.
pub fn load(dir: &Path) -> StoreResult<Config> {
    let roles_dir = dir.join("roles");
    if !roles_dir.is_dir() {
        return Err(cfg_err(format!("no roles/ under {}", dir.display())));
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(&roles_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut roles = Vec::new();
    for p in &paths {
        roles.push(parse_yaml::<RoleFile>(p)?);
    }

    let mut criteria = Vec::new();
    let cdir = dir.join("criteria");
    if cdir.is_dir() {
        let mut cps: Vec<PathBuf> = fs::read_dir(&cdir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
            .collect();
        cps.sort();
        for p in &cps {
            criteria.push(parse_yaml::<Criteria>(p)?);
        }
    }

    let common: Common = parse_yaml(&dir.join("prompts/common.yaml"))?;
    let profiles = load_profiles(&dir.join("evaluation"))?;
    Ok(Config {
        roles,
        criteria,
        common,
        profiles,
    })
}

/// Upsert config roles into the store, versioning on content change.
/// Returns how many role versions were created.
pub fn seed_roles(store: &Store, cfg: &Config, now: Timestamp) -> StoreResult<usize> {
    let mut changed = 0;
    for rf in &cfg.roles {
        let rendered = cfg.build_role(rf, now)?;
        let latest = store.get_role(&rf.slug)?;
        let version = match &latest {
            None => rendered.version,
            Some(cur) => {
                let same = cur.name == rendered.name
                    && cur.purpose == rendered.purpose
                    && cur.instructions == rendered.instructions
                    && cur.done_criteria == rendered.done_criteria
                    && cur.tools == rendered.tools
                    && cur.input_contract == rendered.input_contract
                    && cur.output_contract == rendered.output_contract;
                if same {
                    continue;
                }
                cur.version + 1
            }
        };
        let mut role = rendered.clone();
        role.version = version;
        store.put_role(&role)?;
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn compiles_criteria_into_instructions() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(dir.path()).unwrap();
        let cfg = load(dir.path()).unwrap();
        let rf = cfg
            .roles
            .iter()
            .find(|r| r.slug == "requirements-reviewer")
            .unwrap();
        let role = cfg.build_role(rf, Utc::now()).unwrap();
        assert!(
            role.instructions.contains("[RV1|blocker]"),
            "{}",
            role.instructions
        );
        assert!(role.instructions.contains("Verdict rule:"));
        assert!(role.instructions.contains("Workflow rules:"));
        // Analyst has no criteria — only the common rules block.
        let arf = cfg
            .roles
            .iter()
            .find(|r| r.slug == "requirements-analyst")
            .unwrap();
        let arole = cfg.build_role(arf, Utc::now()).unwrap();
        assert!(!arole.instructions.contains("Verdict rule:"));
        assert!(arole.instructions.contains("Workflow rules:"));
        assert_eq!(
            arole.output_contract.as_ref().unwrap().name,
            "requirements-specification"
        );
    }

    /// QA return7 п.(г): a malformed role `contract:` must surface as a
    /// config error at seed time — never silently degrade to `None` and
    /// leave the role running without its output gate.
    #[test]
    fn malformed_contract_ref_is_a_loud_config_error() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(dir.path()).unwrap();
        let cfg = load(dir.path()).unwrap();
        let mut rf = cfg
            .roles
            .iter()
            .find(|r| r.slug == "requirements-analyst")
            .unwrap()
            .clone();
        rf.output_contract = Some("requirements-specification@notanumber".into());
        let err = cfg.build_role(&rf, Utc::now()).unwrap_err();
        assert!(
            format!("{err}").contains("contract"),
            "the error names the field: {err}"
        );
        // A well-formed ref still compiles.
        rf.output_contract = Some("requirements-specification@2".into());
        let role = cfg.build_role(&rf, Utc::now()).unwrap();
        assert_eq!(role.output_contract.unwrap().version, 2);
    }

    #[test]
    fn factory_profile_loads_valid_and_compiles_into_auditor() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(dir.path()).unwrap();
        let cfg = load(dir.path()).unwrap();
        // The factory profile passes grammar validation on load.
        assert_eq!(cfg.profiles.len(), 1);
        let p = &cfg.profiles[0];
        assert_eq!(p.project, "devai");
        assert_eq!(p.gates.len(), 4);
        assert!(p.checks().iter().any(|c| c.id == "arch/principles"));
        // Floors are derived from error_type severity, never declared per check.
        let lb = p
            .checks()
            .into_iter()
            .find(|c| c.id == "arch/layer-boundaries")
            .unwrap();
        assert_eq!(p.worst_severity(lb), Some("catastrophic"));
        assert_eq!(p.floor_for(lb), Some(0.7));
        // Auditor instructions carry the profile-compiled rubric checklist.
        let rf = cfg
            .roles
            .iter()
            .find(|r| r.slug == "quality-auditor")
            .unwrap();
        let role = cfg.build_role(rf, Utc::now()).unwrap();
        assert!(role.instructions.contains("Evaluation profile `devai`"));
        assert!(
            role.instructions.contains("[arch/idiom-consistency]"),
            "{}",
            role.instructions
        );
        assert!(role.instructions.contains("anchors: 1.0"));
        // Human-signed checks are named but excluded from agent scoring.
        assert!(role.instructions.contains("Human-signed checks"));
        assert!(role.instructions.contains("arch/principles"));
        // Mechanical checks never appear in the agent's checklist.
        assert!(!role.instructions.contains("[corr/transitions] (mechanical"));
        // Flat criteria no longer apply to the auditor (profile supersedes).
        assert!(!role.instructions.contains("Verdict rule:"));
    }

    #[test]
    fn profile_validation_rejects_broken_grammar() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(dir.path()).unwrap();
        let mut cfg = load(dir.path()).unwrap();
        let p = cfg.profiles.first_mut().unwrap();
        // V7: dropping one weight breaks the sum and the coverage set.
        let restored = p
            .scoring
            .weights
            .remove("arch/no-magic")
            .expect("weight present");
        assert!(p.validate().unwrap_err().contains("V7"));
        // V1: an undeclared detected type is rejected.
        p.scoring.weights.insert("arch/no-magic".into(), restored);
        let c = p.evaluations[0].checks[0].clone();
        p.evaluations[0].checks[0].detects = vec!["nonexistent/type".into()];
        assert!(p.validate().unwrap_err().contains("V1"));
        p.evaluations[0].checks[0] = c;
        assert!(p.validate().is_ok(), "restored profile is valid again");
    }

    #[test]
    fn reseed_bumps_version_only_on_change_and_never_deletes() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(&dir.path().join("config")).unwrap();
        let cfg = load(&dir.path().join("config")).unwrap();
        let store = Store::open(dir.path()).unwrap();
        let now = Utc::now();
        assert_eq!(seed_roles(&store, &cfg, now).unwrap(), 6);
        assert_eq!(
            seed_roles(&store, &cfg, now).unwrap(),
            0,
            "idempotent re-seed"
        );

        // The factory YAML declares its own role version (71d416b moved
        // quality-auditor to @2): the assertions track THAT declared number,
        // never a hardcoded literal that rots on the next declarative bump.
        let declared = cfg
            .roles
            .iter()
            .find(|r| r.slug == "quality-auditor")
            .unwrap()
            .version;

        // Edit the evaluation profile (raise theta) → next seed bumps just the auditor.
        let mut cfg2 = cfg.clone();
        cfg2.profiles.first_mut().unwrap().scoring.theta = 0.85;
        assert_eq!(seed_roles(&store, &cfg2, now).unwrap(), 1);

        let q = store.get_role("quality-auditor").unwrap().unwrap();
        assert_eq!(
            q.version,
            declared + 1,
            "latest version wins for new executions"
        );
        assert!(q.instructions.contains("theta 0.85"), "{}", q.instructions);
        // The factory-declared row remains in the table — history is never overwritten.
        let all = store.list_roles().unwrap();
        assert!(
            all.iter()
                .any(|r| r.slug == "quality-auditor" && r.version == declared)
        );
        assert!(
            all.iter()
                .any(|r| r.slug == "quality-auditor" && r.version == declared + 1)
        );
    }

    #[test]
    fn does_not_clobber_user_edits() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        materialize_if_absent(&cfgdir).unwrap();
        let edited = cfgdir.join("criteria/review.yaml");
        fs::write(&edited, "different").unwrap();
        materialize_if_absent(&cfgdir).unwrap();
        assert_eq!(fs::read_to_string(&edited).unwrap(), "different");
    }

    /// REQ-FG-05 AC1 — the schema-authority audit: for every CURRENT contract
    /// version (the latest per name; older stay in the store as history),
    /// each `required` key MUST be declared in `properties` at the same level,
    /// recursively. The precedent violation was requirements-specification@1:
    /// `business_description` required but undeclared — "the validator wants
    /// more than the producer sees", costing full role re-runs (3× '$.
    /// business_description: missing required key'). A reply-shape contract
    /// may not hide expectations behind a `required` list alone.
    #[test]
    fn current_contracts_declare_every_required_key() {
        // Keep only the latest version per name — @1 stays seeded untouched.
        let mut latest: BTreeMap<&str, (u32, &str)> = BTreeMap::new();
        for (name, version, schema) in crate::seed::contracts() {
            let e = latest.entry(name).or_insert((0, ""));
            if version > e.0 {
                *e = (version, schema);
            }
        }
        fn walk(label: &str, node: &serde_json::Value, bad: &mut Vec<String>) {
            let Some(obj) = node.as_object() else { return };
            if let Some(req) = obj.get("required").and_then(|r| r.as_array()) {
                let props = obj.get("properties").and_then(|p| p.as_object());
                for k in req.iter().filter_map(|r| r.as_str()) {
                    if !props.is_some_and(|p| p.contains_key(k)) {
                        bad.push(format!(
                            "{label}: required key `{k}` is not declared in properties"
                        ));
                    }
                }
            }
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                for (k, sub) in props {
                    walk(&format!("{label}.{k}"), sub, bad);
                }
            }
            if let Some(items) = obj.get("items") {
                walk(&format!("{label}[]"), items, bad);
            }
        }
        let mut bad = Vec::new();
        for (name, (version, schema)) in &latest {
            let v: serde_json::Value = serde_json::from_str(schema)
                .unwrap_or_else(|e| panic!("{name}@{version} must parse: {e}"));
            walk(&format!("{name}@{version}"), &v, &mut bad);
        }
        assert!(
            bad.is_empty(),
            "contract schema-authority audit failed:\n{}",
            bad.join("\n")
        );
    }

    /// The ratified reply-shape of @2: echo fields the platform stamps are no
    /// longer hidden obligations on the producer (REQ-FG-06 / OQ-FG-07) — and
    /// the historical incident key is declared in properties now.
    #[test]
    fn requirements_specification_v2_makes_echo_fields_optional_and_declared() {
        let (_, v2, schema) = crate::seed::contracts()
            .into_iter()
            .find(|(name, ver, _)| *name == "requirements-specification" && *ver == 2)
            .expect("requirements-specification@2 is seeded");
        let doc: serde_json::Value = serde_json::from_str(&schema).unwrap();
        let required: Vec<&str> = doc["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        assert!(
            !required.contains(&"task_id"),
            "task_id is platform-stamped, not role-carried"
        );
        assert!(
            !required.contains(&"business_description"),
            "business_description is platform-completed when skipped"
        );
        assert!(
            required.contains(&"version"),
            "version stays role-owned (rework semantics)"
        );
        for k in [
            "task_id",
            "business_description",
            "source",
            "open_questions",
            "change_history",
        ] {
            assert!(
                doc["properties"][k].is_object(),
                "{k} declared in properties"
            );
        }
    }

    /// REQ-FG-04: the loop budget is declarative config, not code — the
    /// factory file materialises, an edit changes it, a home without the file
    /// falls back to the same factory default, and a broken declaration is
    /// loud. `rework-context.yaml` in the same directory must not disturb it.
    #[test]
    fn response_form_budget_is_declarative_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        materialize_if_absent(&cfgdir).unwrap();
        let wf = cfgdir.join("workflow");
        // Factory value: the ratified default (OQ-FG-01), 2 re-prompts.
        let pol = load_response_form_policy(&wf).unwrap();
        assert_eq!(pol.max_repair_attempts, 2, "factory budget");
        // A home predating the file: same factory bytes as fallback.
        fs::remove_file(wf.join("response-form-loop.yaml")).unwrap();
        assert_eq!(
            load_response_form_policy(&wf).unwrap().max_repair_attempts,
            2
        );
        fs::remove_dir_all(&wf).unwrap();
        assert_eq!(
            load_response_form_policy(&wf).unwrap().max_repair_attempts,
            2,
            "absent dir is not an error"
        );
        // Operator edit — no rebuild (REQ-FG-04 AC1).
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("response-form-loop.yaml"),
            "response_form:\n  max_repair_attempts: 0\n",
        )
        .unwrap();
        assert_eq!(
            load_response_form_policy(&wf).unwrap().max_repair_attempts,
            0,
            "0 is valid: loop off, today's behavior"
        );
        // A non-participating workflow file is left alone (rework-context.yaml
        // is a top-level list; it carries no response_form section).
        fs::write(wf.join("other-list.yaml"), "- role: x\n  triggers: []\n").unwrap();
        assert_eq!(
            load_response_form_policy(&wf).unwrap().max_repair_attempts,
            0
        );
        // A broken declaration is a loud config error, never a silent default.
        fs::write(
            wf.join("response-form-loop.yaml"),
            "response_form:\n  max_repair_attempts: -1\n",
        )
        .unwrap();
        assert!(
            load_response_form_policy(&wf)
                .unwrap_err()
                .to_string()
                .contains("max_repair_attempts")
        );
    }

    /// R-007 AC1 + one-table invariant: `init` materialises the policy file,
    /// the home file and the embedded factory bytes read as ONE table, and
    /// the coverage block holds: quality.returned + qa.rework_gate_tripped
    /// feed the implementation role, `qa.rework_gate_incomparable` has NO
    /// trigger (ratified OQ-2 — it carries no human rationale).
    #[test]
    fn rework_policy_materialises_and_the_loader_reads_the_factory_table() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        let written = materialize_if_absent(&cfgdir).unwrap();
        assert!(
            written
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "rework-context.yaml")),
            "init must materialise the REWORK-context policy: {written:?}"
        );
        let loaded = load_rework_policies(&cfgdir.join("workflow")).unwrap();
        assert_eq!(
            loaded,
            factory_rework_policies(),
            "home file and embedded factory bytes are one table, not two"
        );
        let impl_role = loaded
            .iter()
            .find(|p| p.role == "implementation-agent")
            .expect("implementation-agent row");
        let gate = impl_role
            .triggers
            .iter()
            .find(|t| t.event == "qa.rework_gate_tripped")
            .expect("gate trigger declared in factory policy");
        assert_eq!(gate.rationale_field, "qa_return_rationale");
        assert!(
            !gate.attach_findings,
            "the gate quotes no findings — only the QA return flow does"
        );
        let returned = impl_role
            .triggers
            .iter()
            .find(|t| t.event == "quality.returned")
            .unwrap();
        assert!(returned.attach_findings);
        assert!(
            !impl_role
                .triggers
                .iter()
                .any(|t| t.event == "qa.rework_gate_incomparable"),
            "OQ-2: qa.rework_gate_incomparable deliberately feeds nothing"
        );
        // Removing the file re-reads the SAME table from embedded bytes.
        fs::remove_dir_all(cfgdir.join("workflow")).unwrap();
        assert!(
            load_rework_policies(&cfgdir.join("workflow"))
                .unwrap()
                .is_empty(),
            "absent dir is the caller's fallback signal, not an error"
        );
        assert_eq!(
            factory_rework_policies(),
            loaded,
            "fallback is the factory bytes themselves"
        );
    }

    /// Every human-authored selector is checked against the platform's own
    /// vocabularies at load; each misspelling is an error naming file+field.
    #[test]
    fn rework_policy_rejects_every_malformed_selector_loudly() {
        let dir = tempfile::TempDir::new().unwrap();
        let wf = dir.path().join("workflow");
        fs::create_dir_all(&wf).unwrap();
        let write = |body: &str| fs::write(wf.join("zz-policy.yaml"), body).unwrap();
        // unknown event kind — a never-written kind silently matches nothing
        write(
            "- role: implementation-agent\n  triggers:\n    - event: quality.retruned\n      rationale_field: rationale\n",
        );
        let err = load_rework_policies(&wf).unwrap_err().to_string();
        assert!(
            err.contains("quality.retruned") && err.contains("zz-policy.yaml"),
            "{err}"
        );
        // unknown rationale field — would silently read NOTHING
        write(
            "- role: implementation-agent\n  triggers:\n    - event: quality.returned\n      rationale_field: rational\n",
        );
        let err = load_rework_policies(&wf).unwrap_err().to_string();
        assert!(err.contains("rationale_field"), "{err}");
        // unknown verdict field
        write(
            "- role: implementation-agent\n  triggers:\n    - event: review.recorded\n      rationale_field: summary\n      verdict_field: verdikt\n      only_if_verdict_not: APPROVED\n",
        );
        assert!(
            load_rework_policies(&wf)
                .unwrap_err()
                .to_string()
                .contains("verdict_field")
        );
        // broken literal — not a verdict at all (ratified 'APPR0VED' example)
        write(
            "- role: implementation-agent\n  triggers:\n    - event: review.recorded\n      rationale_field: summary\n      only_if_verdict_not: APPR0VED\n",
        );
        assert!(
            load_rework_policies(&wf)
                .unwrap_err()
                .to_string()
                .contains("not a Verdict")
        );
        // parses but non-canonical: the stored-text comparison would never match
        write(
            "- role: implementation-agent\n  triggers:\n    - event: review.recorded\n      rationale_field: summary\n      only_if_verdict_not: approved\n",
        );
        assert!(
            load_rework_policies(&wf)
                .unwrap_err()
                .to_string()
                .contains("canonical")
        );
        // duplicate event within one role row
        write(
            "- role: implementation-agent\n  triggers:\n    - event: quality.returned\n      rationale_field: rationale\n    - event: quality.returned\n      rationale_field: rationale\n",
        );
        assert!(
            load_rework_policies(&wf)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        // empty triggers row
        write("- role: implementation-agent\n  triggers: []\n");
        assert!(
            load_rework_policies(&wf)
                .unwrap_err()
                .to_string()
                .contains("triggers` is empty")
        );
        // empty role slug
        write(
            "- role: \"\"\n  triggers:\n    - event: quality.returned\n      rationale_field: rationale\n",
        );
        assert!(
            load_rework_policies(&wf)
                .unwrap_err()
                .to_string()
                .contains("empty `role`")
        );
    }

    /// Two rows for one role across files would make delivery order depend on
    /// directory listing — rejected, naming BOTH files.
    #[test]
    fn duplicate_rework_role_across_files_names_both_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let wf = dir.path().join("workflow");
        fs::create_dir_all(&wf).unwrap();
        let row = "- role: implementation-agent\n  triggers:\n    - event: quality.returned\n      rationale_field: rationale\n";
        fs::write(wf.join("aa-first.yaml"), row).unwrap();
        fs::write(wf.join("zz-second.yaml"), row).unwrap();
        let err = load_rework_policies(&wf).unwrap_err().to_string();
        assert!(
            err.contains("implementation-agent")
                && err.contains("aa-first.yaml")
                && err.contains("zz-second.yaml"),
            "{err}"
        );
    }

    /// A top-level MAPPING is a different loader's business and must not
    /// break this one; present-but-malformed sequence content IS loud.
    #[test]
    fn rework_loader_skips_mapping_files_and_is_loud_on_malformed_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let wf = dir.path().join("workflow");
        fs::create_dir_all(&wf).unwrap();
        materialize_if_absent(&dir.path().join("config")).unwrap();
        fs::copy(
            dir.path().join("config/workflow/response-form-loop.yaml"),
            wf.join("response-form-loop.yaml"),
        )
        .unwrap();
        assert!(
            load_rework_policies(&wf).unwrap().is_empty(),
            "the mapping file is skipped, not parsed as rows"
        );
        fs::write(wf.join("broken.yaml"), "- not-a-row\n").unwrap();
        let err = load_rework_policies(&wf).unwrap_err().to_string();
        assert!(
            err.contains("broken.yaml") && err.contains("role, triggers"),
            "{err}"
        );
    }

    /// REQ-006 sovereignty, pinned as a fact about the factory tree: role
    /// YAMLs say nothing about REWORK delivery — the mechanism is
    /// platform-owned config plus platform code; roles stay generic.
    #[test]
    fn role_configs_never_mention_rework_delivery() {
        let roles = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/roles");
        let mut seen = 0usize;
        for entry in fs::read_dir(&roles).unwrap() {
            let p = entry.unwrap().path();
            if !p.extension().is_some_and(|x| x == "yaml") {
                continue;
            }
            let text = fs::read_to_string(&p).unwrap();
            assert!(
                !text.contains("REWORK CONTEXT")
                    && !text.contains("rework-context")
                    && !text.contains("rationale_field"),
                "{} leaks the delivery policy into a role config",
                p.display()
            );
            seen += 1;
        }
        assert!(
            seen >= 5,
            "the factory role set must be present (saw {seen} files)"
        );
    }

    #[test]
    fn adds_new_factory_files_to_an_old_home_without_clobbering() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        materialize_if_absent(&cfgdir).unwrap();
        fs::write(cfgdir.join("criteria/review.yaml"), "mine").unwrap();
        // Simulate a home created before the profile existed.
        fs::remove_dir_all(cfgdir.join("evaluation")).unwrap();
        let written = materialize_if_absent(&cfgdir).unwrap();
        assert!(
            written
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "quality.yaml")
                    && p.parent()
                        .is_some_and(|d| d.file_name().is_some_and(|n| n == "evaluation"))),
            "re-init must bring the new factory profile into an old home: {written:?}"
        );
        assert_eq!(
            fs::read_to_string(cfgdir.join("criteria/review.yaml")).unwrap(),
            "mine"
        );
    }
}
