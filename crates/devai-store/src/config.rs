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
//!   evaluation/base.yaml       # factory BASE profile (QA): gates, checks, scoring —
//!                              #   abstract only; project specifics enter through the
//!                              #   ratified overlay delta in `profiles/` (REQ-SP-101/106),
//!                              #   never through `evaluation/` files
//!   roles/<slug>.yaml          # role definitions
//! ```

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use devai_core::inbox::InboxWaitPolicy;
use devai_core::types::{
    ContractRef, Decision, ProfileRatification, RoleDefinition, Timestamp, Verdict, decision_kind,
    event_kind, json_field, new_id,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Store, StoreError, StoreResult};

// Embedded factory defaults — written into the home config on first init.
// REQ-SP-101 (cycle C1): the factory distributes the ABSTRACT BASE profile
// only — no project paths, crate names, single-project commands, no
// platform-invariants block. Project specifics enter force solely through a
// ratified overlay delta (`profiles/<projects.id>.overlay.yaml`, REQ-SP-106),
// which is NOT part of DEFAULT_FILES and never lives in `evaluation/`. The
// devai v11 profile stays in the repo (config/evaluation/quality.yaml) as the
// C5 migration material; per-file materialisation means existing homes keep
// their own untouched copy (config.rs `materialize_if_absent`).
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
        "evaluation/base.yaml",
        include_str!("../../../config/evaluation/base.yaml"),
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
    (
        "workflow/humanize.yaml",
        include_str!("../../../config/workflow/humanize.yaml"),
    ),
    (
        "workflow/task-name.yaml",
        include_str!("../../../config/workflow/task-name.yaml"),
    ),
    (
        "workflow/agent-failure.yaml",
        include_str!("../../../config/workflow/agent-failure.yaml"),
    ),
    (
        "workflow/park-reasons.yaml",
        include_str!("../../../config/workflow/park-reasons.yaml"),
    ),
    (
        "workflow/process.yaml",
        include_str!("../../../config/workflow/process.yaml"),
    ),
    ("models.yaml", include_str!("../../../config/models.yaml")),
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

    /// The profile compiled into a role's checklist. C1 transition rule
    /// (REQ-SP-101 × operator decision "no working profile breaks before
    /// C5"): when a home carries the factory base ALONGSIDE an owned legacy
    /// full profile (an existing home gained `base.yaml` on re-init while
    /// its `quality.yaml` is preserved by per-file materialisation), role
    /// compilation keeps preferring the LEGACY-OWNED profile — the factory
    /// swap must not silently rewrite the auditor's checklist under a home
    /// that has not been migrated to base+overlay (C5). A clean home with
    /// only the base compiles the base (REQ-SP-101 AC2).
    pub fn profile_for(&self, slug: &str) -> Option<&Profile> {
        let mut applicable = self
            .profiles
            .iter()
            .filter(|p| p.applies_to_roles.iter().any(|r| r == slug));
        let mut legacy = applicable.clone();
        legacy
            .find(|p| {
                p.name.as_deref() != Some(FACTORY_BASE_MARKER) && p.project != FACTORY_BASE_MARKER
            })
            .or_else(|| applicable.next())
    }

    /// Compile an applied profile into the role's checklist text: the agent
    /// sees ONLY the checks it may score, with anchors, plus the machine
    /// budget so its reply is shaped for platform assembly.
    fn compile_profile(profile: &Profile, ins: &mut String) {
        use std::fmt::Write as _;
        let report_contract = format!(
            "{}@{}",
            devai_core::types::QUALITY_REPORT_CONTRACT.0,
            devai_core::types::QUALITY_REPORT_CONTRACT.1
        );
        let _ = writeln!(
            ins,
            "\n\nEvaluation profile `{}` v{}: the PLATFORM runs gates and mechanical probes, computes S, floors and the verdict — you never emit a verdict. Your reply is an evaluation-assessment JSON: `scores` ({{check-id: number}} for the rubric/hybrid checks below), `findings` ([{{check, text, evidence[]}}] — text = what + file:line + what it leads to; `evidence` is a NON-EMPTY array of concrete citations: test output, artifact id, build log), `gaps` ([{{check, reason}}] when you could not verify something honestly — an unperformed check is never a fake score). The platform assembles the {report_contract} (bound to the workspace revision captured at execution start), and the contract gate rejects any criteria entry whose evidence array is missing or empty — no criterion is ever recorded as a pass without evidence.",
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
                    "  evidence: REQUIRED — every criteria entry must cite at least one concrete evidence item (test output, artifact id, build log). A score < 1.0 without evidence is discarded (proc/evidence-completeness); a PASS backed only by your judgment is contract-invalid ({report_contract}: minItems 1 per check)."
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
// Project overlay DELTA (REQ-SP-104/106/107, cycle C1) — base + ratified
// specifics -> effective profile.
// ---------------------------------------------------------------------------

/// The factory base profile's name marker (R4). The base is identified by
/// its name — the `base.yaml` file stem, filled by the loader — never by an
/// anonymous first match. `Config::profile_for`'s legacy-preference rule
/// excludes exactly this marker, so "the factory swap must not silently
/// rewrite an unmigrated home's auditor checklist" (REQ-SP-101 transition)
/// is expressed by name and intent, not a bare string literal.
pub const FACTORY_BASE_MARKER: &str = "base";

/// Default section id for a ratified `add` that does not name one (R4):
/// REQ-SP-104 lets adds bring new sections; this is the conventional id
/// the composition gives them.
const OVERLAY_SECTION_ID: &str = "project-overlay";

/// Default section title paired with `OVERLAY_SECTION_ID` (R4).
const OVERLAY_SECTION_TITLE: &str = "Project specifics (ratified overlay)";

/// Typed delta-layer failure (R8): every rejection names the violated rule
/// (`code` — a spec/grammar identifier) and carries the full human text.
/// String wrapping remains ONLY at the message boundary
/// (`resolve_effective_profile` -> `StoreError::Config`); callers can branch
/// on the code instead of matching error strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct OverlayError {
    /// Rule identifier the violation falls under (REQ-SP-106 / REQ-SP-107 /
    /// V1-V8 effective-grammar rejection).
    pub code: &'static str,
    /// Fully rendered human message (includes the offending item where the
    /// rule names one).
    pub message: String,
}

impl OverlayError {
    fn new(code: &'static str, message: String) -> Self {
        OverlayError { code, message }
    }
}

type OverlayResult<T> = Result<T, OverlayError>;

/// One item of the delta file: `add` contributes a new project check with
/// a declared weight. `ratified_by` is the data-field link to the human
/// decision that gives the item force (REQ-SP-106, A-5).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OverlayAddPayload {
    pub statement: String,
    #[serde(default)]
    pub detects: Vec<String>,
    /// mechanical | rubric | hybrid
    pub verify: String,
    #[serde(default)]
    pub probes: Vec<Probe>,
    #[serde(default)]
    pub anchors: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub evidence: String,
    #[serde(default = "either")]
    pub reviewer: String,
    #[serde(default)]
    pub suppression: Option<String>,
    /// Declared weight, ratified per item (REQ-SP-107); 0 < w < 1.
    pub weight: f64,
    /// New error types the added check declares (V1 coverage).
    #[serde(default)]
    pub error_types: BTreeMap<String, ErrorTypeDef>,
    /// Section (evaluation) to place the check in; new sections allowed
    /// (REQ-SP-104 "add may bring new sections").
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub section_title: Option<String>,
}

/// Item kind of the delta (REQ-SP-104/106). A SERDE ENUM, not a free
/// string with a runtime dictionary (R3): an invalid `kind` in the delta
/// file is rejected by the PARSER, before any validation-order logic runs.
/// `rename_all = "lowercase"` keeps the stored dictionary exactly `add`;
/// the overlay can only ADD rules — a retired `bind` value is an unknown
/// variant and fails at parse time.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OverlayKind {
    Add,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OverlayItem {
    pub kind: OverlayKind,
    /// add: a NEW id (never an existing base id).
    pub target_id: String,
    /// id of a decisions row (kind=profile_ratification, verdict=APPROVED)
    /// whose refs cover `target_id`. An EMPTY value is the normal state of a
    /// fresh rules file (RV8): the file loads and the item is announced as
    /// out of force. Force is the cross-check (A-5) — a dangling, foreign,
    /// revoked or edition-stale ref drops the item with a named reason
    /// (`item_forces`, RV2).
    #[serde(default)]
    pub ratified_by: String,
    /// add: the full check payload.
    #[serde(default)]
    pub add: Option<OverlayAddPayload>,
}

/// The delta file: NOT a Profile — parsing it as one must fail, and the
/// composition loader reads it with THIS type only (REQ-SP-106, closing RV2:
/// `evaluation/*.yaml` keeps its "full Profile" semantics untouched).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProfileOverlay {
    /// projects.id the overlay is keyed by (stable PK, not display name).
    pub project_id: String,
    /// The base this delta was authored against, `name@version` — kept as a
    /// provenance signal: the base is matched by NAME, so a version drift is
    /// visible without silently discarding the overlay.
    pub base_identity: String,
    pub overlay_version: u32,
    #[serde(default)]
    pub items: Vec<OverlayItem>,
}

/// Outcome of composing base + ratified items.
#[derive(Clone, Debug)]
pub struct Composition {
    pub effective: Profile,
    /// target_ids of items that entered force.
    pub applied: Vec<String>,
    /// target_ids dropped by the force cross-check (announced loudly —
    /// REQ-SP-106 "не прошедший item не входит в композицию и оглашается").
    pub dropped: Vec<String>,
    /// Non-fatal signals raised while composing (e.g. a weight rescale that
    /// had nothing to scale). Nothing is reset silently; a warning is the
    /// loader's announcement surface.
    pub warnings: Vec<String>,
}

/// The conventional delta path for one project (single file per projects.id
/// — REQ-SP-106 AC "два оверлея на один id невозможны (один путь)").
pub fn overlay_path(cfg_dir: &Path, project_id: &str) -> PathBuf {
    cfg_dir.join(overlay_rel_path(project_id))
}

/// The same path as it is RECORDED in a ratification decision: relative to
/// the config directory, so a home moved to another root keeps a readable
/// provenance without a machine-specific absolute path.
pub fn overlay_rel_path(project_id: &str) -> String {
    format!("profiles/{project_id}.overlay.yaml")
}

/// The EDITION fingerprint of the rules content (ASM-1, RV2): the parsed
/// document in canonical form — items sorted by target_id, every
/// ratification pointer stripped — hashed with SHA-256 as `sha256:<hex>`.
///
/// Why canonical form and not raw bytes: `ratify` itself WRITES the
/// pointers into the file (RV3), so a byte-level digest would invalidate
/// the very act that produced it. Any edit to the RULE content — item
/// text, weight, an added/removed item, the header — changes this
/// fingerprint; rewriting only `ratified_by` (or reordering items, which
/// composition already ignores) does not. That is exactly the force rule
/// RV2 states.
pub fn overlay_edition(overlay: &ProfileOverlay) -> String {
    let mut doc = serde_json::to_value(overlay).unwrap_or(serde_json::Value::Null);
    if let Some(items) = doc.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            if let Some(map) = item.as_object_mut() {
                map.remove("ratified_by");
            }
        }
        items.sort_by(|a, b| {
            let key = |v: &serde_json::Value| {
                v.get("target_id")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            key(a).cmp(&key(b))
        });
    }
    let bytes = serde_json::to_vec(&doc).unwrap_or_default();
    format!("sha256:{}", crate::hex_encode(&Sha256::digest(&bytes)))
}

/// The edition fingerprint of the CURRENT rules file of one project. The
/// command computes it itself (RV1: the human never types a hash), and the
/// human state view prints the same value, so the two can be compared.
pub fn overlay_edition_at(
    cfg_dir: &Path,
    project_id: &str,
) -> StoreResult<(String, ProfileOverlay)> {
    let path = overlay_path(cfg_dir, project_id);
    let overlay = load_overlay(&path).map_err(|e| match e {
        StoreError::Config(m) => cfg_err(format!("{}: {m}", path.display())),
        other => other,
    })?;
    Ok((overlay_edition(&overlay), overlay))
}

/// One stored `profile_ratification` row as the force rule sees it (ASM-3):
/// the typed edition binding, who ratified, the row's verdict and whether the
/// row still gives force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatificationRow {
    pub payload: ProfileRatification,
    pub actor: String,
    pub verdict: Verdict,
    /// An APPROVED row cited by a later revocation row gives no force.
    pub live: bool,
}

/// THE index of stored ratification rows: decision id -> row, with the
/// revocation filter applied exactly ONCE. Composition, the human state view
/// and the reuse key of the ratify act all read this map, so "which
/// ratification is in force" cannot drift between readers.
///
/// A row whose payload does not decode is a VISIBLE corruption: skipping it
/// would make a torn act read as "never ratified" and let the next act
/// silently supersede history. Invalid JSON fails earlier still, at
/// `rows::decision_from_row`.
pub fn ratification_index(rows: &[Decision]) -> StoreResult<BTreeMap<String, RatificationRow>> {
    let ratifications = || {
        rows.iter()
            .filter(|d| d.kind == decision_kind::PROFILE_RATIFICATION)
    };
    let revoked: HashSet<String> = ratifications()
        .filter(|d| d.verdict != Verdict::Approved)
        .filter_map(|d| {
            d.payload
                .get(json_field::REVOKES)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    let mut index = BTreeMap::new();
    for row in ratifications() {
        let payload: ProfileRatification =
            serde_json::from_value(row.payload.clone()).map_err(|e| {
                crate::error::StoreError::Corrupt(format!(
                    "decision {} ({}) carries no ratification binding (project/edition/items): {e}",
                    row.id, row.kind
                ))
            })?;
        let live = row.verdict == Verdict::Approved && !revoked.contains(&row.id.to_string());
        index.insert(
            row.id.to_string(),
            RatificationRow {
                payload,
                actor: row.actor.clone(),
                verdict: row.verdict,
                live,
            },
        );
    }
    Ok(index)
}

/// Force state of ONE overlay item, as the loader sees it. `force` is the
/// live decision that keeps the item in force; `reason` says WHY an item is
/// out — the RV8 vocabulary (empty pointer, unknown decision, wrong kind or
/// verdict, another project, a changed edition, a coverage gap, a
/// revocation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemForce {
    pub force: Option<(String, String)>,
    pub reason: Option<String>,
}

impl ItemForce {
    /// An item that gives no force, carrying the reason a human reads.
    fn out(reason: impl Into<String>) -> ItemForce {
        ItemForce {
            force: None,
            reason: Some(reason.into()),
        }
    }
}

/// Resolve every item of the overlay against the CURRENT edition and the
/// stored ratification index. ONE function feeds composition, the human
/// state view and the loader announcements, so a verdict on an item is
/// stated once and never re-derived differently by another reader.
pub fn item_forces(
    overlay: &ProfileOverlay,
    edition: &str,
    ratifications: &BTreeMap<String, RatificationRow>,
) -> BTreeMap<String, ItemForce> {
    let mut out = BTreeMap::new();
    for item in &overlay.items {
        let state = force_of(item, &overlay.project_id, edition, ratifications);
        out.insert(item.target_id.clone(), state);
    }
    out
}

fn force_of(
    item: &OverlayItem,
    project_id: &str,
    edition: &str,
    ratifications: &BTreeMap<String, RatificationRow>,
) -> ItemForce {
    if item.ratified_by.trim().is_empty() {
        return ItemForce::out("нератифицирован: указатель на решение не задан");
    }
    let Some(row) = ratifications.get(&item.ratified_by) else {
        return ItemForce::out(format!(
            "нератифицирован: указатель `{}` не называет решение-ратификацию профиля",
            item.ratified_by
        ));
    };
    if row.verdict != Verdict::Approved {
        return ItemForce::out(format!(
            "нератифицирован: решение `{}` не является APPROVED-ратификацией профиля",
            item.ratified_by
        ));
    }
    if !row.live {
        return ItemForce::out("ратификация отозвана");
    }
    if let Some(state) = binding_outage(&row.payload, project_id, edition, item) {
        return state;
    }
    ItemForce {
        force: Some((item.ratified_by.clone(), row.actor.clone())),
        reason: None,
    }
}

/// The RV2/RV7 binding check of ONE live ratification: `Some(out-of-force
/// state)` when the binding does not cover THIS item, `None` when it does.
/// The three reasons are the vocabulary the loader announcement, the human
/// state view and the CLI print, so they are spelled exactly ONCE.
fn binding_outage(
    payload: &ProfileRatification,
    project_id: &str,
    edition: &str,
    item: &OverlayItem,
) -> Option<ItemForce> {
    if payload.project_id != project_id {
        return Some(ItemForce::out(format!(
            "нератифицирован: решение `{}` ратифицирует правила другого проекта",
            item.ratified_by
        )));
    }
    if payload.edition != edition {
        return Some(ItemForce::out("редакция изменена после ратификации"));
    }
    if !payload.items.iter().any(|i| i == &item.target_id) {
        return Some(ItemForce::out("решение не покрывает этот пункт"));
    }
    None
}

/// Read the delta (layer (a) of REQ-SP-106). Structural validation of the
/// items runs at composition (`validate_overlay`): an add colliding with a
/// base id or a sibling, or a weight out of (0,1), is a hard error naming
/// the item. An EMPTY `ratified_by` is deliberately NOT a load error (RV8):
/// a fresh rules file loads, and composition drops + announces the item.
pub fn load_overlay(path: &Path) -> StoreResult<ProfileOverlay> {
    let overlay: ProfileOverlay = parse_yaml(path)?;
    Ok(overlay)
}

/// Write the ratification POINTERS into the rules file (RV3): every covered
/// item gets `ratified_by: <decision id>`. This is the second half of the
/// human act; the fingerprint is computed over the document WITHOUT these
/// pointers, so writing them does not create a new edition.
///
/// Block-style item lists are edited line-by-line, so a human's comments and
/// formatting survive the act; a flow-style file (one `{...}` mapping per
/// item) falls back to a canonical re-serialisation of the parsed document.
pub fn set_ratification_pointers(
    path: &Path,
    decision_id: &str,
    target_ids: &[String],
) -> StoreResult<()> {
    let text =
        fs::read_to_string(path).map_err(|e| cfg_err(format!("read {}: {e}", path.display())))?;
    let updated = rewrite_pointers(&text, decision_id, target_ids)
        .or_else(|| rewrite_pointers_structured(&text, decision_id, target_ids));
    let Some(updated) = updated else {
        return Err(cfg_err(format!(
            "{}: cannot place the ratification pointer for every covered item — the file does not parse as an overlay or an item is missing",
            path.display()
        )));
    };
    write_atomically(path, &updated)
        .map_err(|e| cfg_err(format!("write {}: {e}", path.display())))?;
    Ok(())
}

/// Replace the file through a SIBLING temp file + rename (RV3 AC1). A plain
/// `fs::write` truncates first, so a failure mid-write leaves a half rules
/// file — and the act's promise is that a failed pointer half gives items no
/// force, not that it eats the neighbouring rules. `rename` is atomic within
/// a directory: the reader sees the previous edition or the new one, never a
/// prefix of either. A failed rename removes the temp file so the next
/// attempt is not wedged by a leftover.
fn write_atomically(path: &Path, body: &str) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "overlay".into());
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Line-preserving pointer edit for block-style item lists. `None` when an
/// item start or a covered target cannot be located (the caller then falls
/// back to the structured rewrite).
fn rewrite_pointers(text: &str, decision_id: &str, targets: &[String]) -> Option<String> {
    let wanted: HashSet<&str> = targets.iter().map(String::as_str).collect();
    let mut done: HashSet<String> = HashSet::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len() + 64);
    let mut i = 0;
    while i < lines.len() {
        if !is_item_start(lines[i]) {
            out.push_str(lines[i]);
            i += 1;
            continue;
        }
        let end = item_block_end(&lines, i);
        out.push_str(&rewrite_item_block(
            &lines[i..end],
            decision_id,
            &wanted,
            &mut done,
        ));
        i = end;
    }
    if wanted.iter().all(|t| done.contains(*t)) {
        Some(out)
    } else {
        None
    }
}

/// A block-style item opens with `- kind:`; the dash may be indented, so the
/// test runs on the trimmed line (the indent is read again by the block end).
fn is_item_start(line: &str) -> bool {
    line.trim_start()
        .strip_prefix('-')
        .map(|rest| rest.trim_start().starts_with("kind:"))
        .unwrap_or(false)
}

/// The exclusive end of the item block that starts at `start`: the first
/// following line indented no deeper than the dash. Blank lines do not end a
/// block (a human may separate the item's fields), so a stray blank line
/// never splits an item in two.
fn item_block_end(lines: &[&str], start: usize) -> usize {
    let dash_indent = lines[start].len() - lines[start].trim_start().len();
    let mut j = start + 1;
    while j < lines.len() {
        let l = lines[j];
        let lt = l.trim_start();
        if lt.trim().is_empty() {
            j += 1;
            continue;
        }
        if l.len() - lt.len() <= dash_indent {
            break;
        }
        j += 1;
    }
    j
}

fn rewrite_item_block(
    block: &[&str],
    decision_id: &str,
    wanted: &HashSet<&str>,
    done: &mut HashSet<String>,
) -> String {
    let Some((ti, pointer)) = item_field_lines(block) else {
        return block.concat();
    };
    let raw = field_value(block[ti]);
    let hit = wanted.contains(raw.as_str());
    if hit {
        done.insert(raw);
    }
    let pointer_line = format!("{}ratified_by: {decision_id}\n", indent_of(block[ti]));
    let mut out = String::new();
    for (k, l) in block.iter().enumerate() {
        if k == ti {
            out.push_str(l);
            // No `ratified_by` field yet: the pointer is inserted right after
            // `target_id`, where a human would have written it.
            if hit && pointer.is_none() {
                if !l.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&pointer_line);
            }
        } else if Some(k) == pointer && hit {
            out.push_str(&pointer_line);
        } else {
            out.push_str(l);
        }
    }
    out
}

/// The leading whitespace of a YAML line — the indent a written pointer
/// inherits so it stays inside its item block.
fn indent_of(line: &str) -> String {
    line.chars().take_while(|c| c.is_whitespace()).collect()
}

/// The `target_id` line and the `ratified_by` line of one item block, if the
/// block has the former. `None` means "not an addressable item" — the caller
/// copies the block verbatim instead of inventing a pointer position.
fn item_field_lines(block: &[&str]) -> Option<(usize, Option<usize>)> {
    let mut target: Option<usize> = None;
    let mut pointer: Option<usize> = None;
    for (k, l) in block.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("target_id:") {
            target = Some(k);
        } else if t.starts_with("ratified_by:") {
            pointer = Some(k);
        }
    }
    target.map(|ti| (ti, pointer))
}

/// The scalar value of a `key: value` line, with optional quotes stripped.
fn field_value(line: &str) -> String {
    line.split_once(':')
        .map(|(_, v)| v.trim().trim_matches(|c| c == '"' || c == '\''))
        .unwrap_or("")
        .to_string()
}

/// Canonical rewrite fallback: parse, set the pointers, re-serialise. Used
/// for flow-style item lists that a line edit cannot address.
fn rewrite_pointers_structured(
    text: &str,
    decision_id: &str,
    targets: &[String],
) -> Option<String> {
    let mut overlay: ProfileOverlay = serde_yaml::from_str(text).ok()?;
    if overlay.items.is_empty() {
        return None;
    }
    for item in overlay.items.iter_mut() {
        if targets.iter().any(|t| t == &item.target_id) {
            item.ratified_by = decision_id.to_string();
        }
    }
    serde_yaml::to_string(&overlay).ok()
}

fn base_target_ids(base: &Profile) -> HashSet<&str> {
    let mut ids: HashSet<&str> = HashSet::new();
    for g in &base.gates {
        ids.insert(g.id.as_str());
    }
    for c in base.checks() {
        ids.insert(c.id.as_str());
    }
    ids
}

/// Structural validation of the delta against the base (layer (a)).
/// Typed failures (R8): each names its rule code; the `kind` dictionary
/// itself is enforced by the serde enum at PARSE time (R3), so no runtime
/// "unknown kind" branch survives here.
pub fn validate_overlay(overlay: &ProfileOverlay, base: &Profile) -> OverlayResult<()> {
    let base_ids = base_target_ids(base);
    let mut seen: HashSet<&str> = HashSet::new();
    for item in &overlay.items {
        let who = format!("overlay item `{}`", item.target_id);
        // RV8: an EMPTY `ratified_by` is a NORMAL state of a fresh rules
        // file, not a structural load failure — the item is dropped and
        // announced by composition. Structural faults of the item itself
        // (id collision, weight out of range, duplicate) still refuse.
        match item.kind {
            OverlayKind::Add => {
                if base_ids.contains(item.target_id.as_str()) {
                    return Err(OverlayError::new(
                        "REQ-SP-104",
                        format!(
                            "{who}: add target collides with an existing base id — a new project check must carry a NEW id (REQ-SP-104/V2)"
                        ),
                    ));
                }
                let Some(a) = &item.add else {
                    return Err(OverlayError::new(
                        "REQ-SP-106",
                        format!("{who}: kind=add requires the `add` payload"),
                    ));
                };
                if !(0.0 < a.weight && a.weight < 1.0) {
                    return Err(OverlayError::new(
                        "REQ-SP-106",
                        format!(
                            "{who}: declared weight {} must be 0 < w < 1 (REQ-SP-106)",
                            a.weight
                        ),
                    ));
                }
            }
        }
        if !seen.insert(item.target_id.as_str()) {
            return Err(OverlayError::new(
                "REQ-SP-106",
                format!("{who}: duplicate target_id among overlay items"),
            ));
        }
    }
    Ok(())
}

/// Compose the effective profile: base + the RATIFIED subset of the delta.
/// `ratifications` are the STORED `profile_ratification` decision rows
/// (APPROVED ones and their revocations). An item enters force only when
/// its `ratified_by` resolves to a LIVE APPROVED ratification that (a)
/// belongs to THIS project, (b) records the CURRENT edition fingerprint of
/// the rules file, and (c) covers the item id (task 01a1bacf, RV2/RV7/RV8).
/// Every other item is DROPPED and announced with its own reason — a
/// manual edit without a valid decision reference never enters force.
///
/// Weights follow REQ-SP-107 exactly: W = Σ ratified add weights;
/// W >= 1 rejects the composition naming the items; otherwise base weights
/// scale by (1−W) and adds carry their declared weight. Application order
/// is sorted by target id, so one base@version + overlay@version yields a byte-identical effective
/// profile (REQ-SP-103 AC) regardless of file order. The overlay ADDS only:
/// it contributes new checks, never rewrites a base rule.
pub fn compose_profile(
    base: &Profile,
    overlay: &ProfileOverlay,
    ratifications: &BTreeMap<String, RatificationRow>,
) -> OverlayResult<Composition> {
    validate_overlay(overlay, base)?;
    let edition = overlay_edition(overlay);
    let forces = item_forces(overlay, &edition, ratifications);

    let mut applied: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Deterministic processing order: items sorted by target_id.
    let mut ordered: Vec<&OverlayItem> = overlay.items.iter().collect();
    ordered.sort_by(|a, b| a.target_id.cmp(&b.target_id));
    let in_force: Vec<&OverlayItem> = ordered
        .into_iter()
        .filter(|item| {
            let force = forces
                .get(&item.target_id)
                .is_some_and(|f| f.force.is_some());
            if force {
                applied.push(item.target_id.clone());
            } else {
                dropped.push(item.target_id.clone());
                let reason = forces
                    .get(&item.target_id)
                    .and_then(|f| f.reason.clone())
                    .unwrap_or_else(|| "нератифицирован".to_string());
                warnings.push(format!(
                    "overlay item `{}` NOT in force: {reason} — project judged without it (A-5)",
                    item.target_id
                ));
            }
            force
        })
        .collect();

    let mut eff = base.clone();

    // Adds: new project checks under NEW ids, with their declared weights
    // and any new error types / sections (REQ-SP-104).
    let mut add_weights: Vec<(String, f64)> = Vec::new();
    for item in in_force.iter().filter(|i| i.kind == OverlayKind::Add) {
        let who = format!("add `{}`", item.target_id);
        let a = item.add.as_ref().ok_or_else(|| {
            OverlayError::new(
                "REQ-SP-106",
                format!("{who}: kind=add requires the `add` payload"),
            )
        })?;
        for (t, def) in &a.error_types {
            match eff.error_types.get(t) {
                Some(existing)
                    if existing.desc == def.desc && existing.severity == def.severity => {}
                Some(_) => {
                    return Err(OverlayError::new(
                        "REQ-SP-104",
                        format!(
                            "{who}: error type `{t}` collides with a different declaration in the base"
                        ),
                    ));
                }
                None => {
                    eff.error_types.insert(t.clone(), def.clone());
                }
            }
        }
        let check = CheckDef {
            id: item.target_id.clone(),
            statement: a.statement.clone(),
            detects: a.detects.clone(),
            verify: a.verify.clone(),
            probes: a.probes.clone(),
            anchors: a.anchors.clone(),
            evidence: a.evidence.clone(),
            reviewer: a.reviewer.clone(),
            suppression: a.suppression.clone(),
        };
        let section = a
            .section
            .clone()
            .unwrap_or_else(|| OVERLAY_SECTION_ID.into());
        if let Some(e) = eff.evaluations.iter_mut().find(|e| e.id == section) {
            e.checks.push(check);
        } else {
            eff.evaluations.push(EvaluationDef {
                id: section.clone(),
                title: a
                    .section_title
                    .clone()
                    .unwrap_or_else(|| OVERLAY_SECTION_TITLE.into()),
                applies_to: None,
                checks: vec![check],
            });
        }
        add_weights.push((item.target_id.clone(), a.weight));
    }

    // Weights (REQ-SP-107): proportional rescaling of the base preserves
    // its hierarchy; only the declared add weights are human-ratified
    // numbers. Sums run over sorted ids so the result is order-independent.
    let w_add: f64 = add_weights.iter().map(|(_, w)| w).sum();
    if w_add >= 1.0 {
        let ids: Vec<String> = add_weights.iter().map(|(i, _)| i.clone()).collect();
        return Err(OverlayError::new(
            "REQ-SP-107",
            format!(
                "overlay rejected (REQ-SP-107): ratified add weights sum {w_add} >= 1 — items: {}",
                ids.join(", ")
            ),
        ));
    }
    if w_add > 0.0 {
        // `eff.scoring.weights` still holds ONLY base weights here (adds
        // are inserted below), so scaling every entry scales the base.
        for w in eff.scoring.weights.values_mut() {
            *w *= 1.0 - w_add;
        }
    }
    for (id, w) in add_weights {
        eff.scoring.weights.insert(id, w);
    }

    // Grammar V1-V8 checks the EFFECTIVE profile (REQ-SP-106: not each
    // file separately); V2 uniqueness covers base ids + applied adds.
    eff.validate().map_err(|e| {
        OverlayError::new(
            "V1-V8",
            format!("effective profile invalid after composition: {e}"),
        )
    })?;

    Ok(Composition {
        effective: eff,
        applied,
        dropped,
        warnings,
    })
}

/// Outcome of resolving the factory layers for one task/project: the
/// effective profile plus the loader's announcements (dropped non-ratified
/// items — REQ-SP-106 "оглашается загрузчиком").
#[derive(Clone, Debug)]
pub struct Resolution {
    pub profile: Option<Profile>,
    pub warnings: Vec<String>,
    /// target_id -> (decision id, actor) for the RATIFIED overlay items that
    /// actually entered force in this composition. The human form of a
    /// quality report names the anchoring decision from here (OQ-QH-2);
    /// resolving that at read time instead would be a second derivation.
    pub ratified: BTreeMap<String, (String, String)>,
}

/// Deterministic per-project resolution (REQ-SP-103, replaces the old
/// "first profile that mentions quality-auditor"):
/// 1. the task's project id selects the delta `profiles/<projects.id>.overlay.yaml`;
///    its `base_identity` names the base inside `evaluation/`. Force is decided
///    by the decisions cross-check, so C1-without-C3 (no `profile_ratification`
///    rows yet) composes to the PURE BASE — expected intermediate behavior (A-5).
/// 2. without an overlay: a profile OWNED by the project (`project:` equals the
///    projects.name — the pre-overlay full-profile path, kept working for the
///    existing v11 home until C5 migrates it; a task of project A never runs
///    under project B's profile), else the factory base — the first
///    quality-auditor profile of the sorted evaluation/ namespace.
pub fn resolve_effective_profile(
    store: &Store,
    cfg_dir: &Path,
    project_id: Option<&str>,
) -> StoreResult<Resolution> {
    let profiles = load_profiles(&cfg_dir.join("evaluation"))?;
    let applies = |p: &&Profile| p.applies_to_roles.iter().any(|r| r == "quality-auditor");
    // A task WITHOUT a project: the most MATURE auditor court the home carries
    // wins, version ties resolved by the sorted load order. On a clean home
    // that is just the factory base (REQ-SP-101); on a home that still carries
    // an older owned full profile, that profile keeps judging (operator
    // decision: init must not swap a working checklist for the fresh base
    // before the home is migrated).
    let fallback = profiles
        .iter()
        .filter(applies)
        .fold(None::<&Profile>, |best, p| match best {
            Some(b) if b.version >= p.version => Some(b),
            _ => Some(p),
        });
    let Some(pid) = project_id else {
        // No project: judged on the base (a clean home carries exactly it).
        return Ok(Resolution {
            profile: fallback.cloned(),
            warnings: Vec::new(),
            ratified: BTreeMap::new(),
        });
    };
    let path = overlay_path(cfg_dir, pid);
    if path.is_file() {
        let overlay = load_overlay(&path).map_err(|e| match e {
            StoreError::Config(m) => cfg_err(format!("{}: {m}", path.display())),
            other => other,
        })?;
        if overlay.project_id != pid {
            return Err(cfg_err(format!(
                "{}: header project_id `{}` does not match the key `{pid}` — the overlay is keyed by projects.id (REQ-SP-103/106)",
                path.display(),
                overlay.project_id
            )));
        }
        // The base is matched by NAME: a version drift in `base_identity`
        // does not discard the overlay — the items' own force rule (A-5)
        // still decides what enters composition.
        let base_name = overlay.base_identity.split('@').next().unwrap_or("");
        let base = profiles
            .iter()
            .find(|p| p.name.as_deref() == Some(base_name) && !p.applies_to_roles.is_empty())
            .or_else(|| {
                profiles
                    .iter()
                    .find(|p| p.name.as_deref() == Some(base_name))
            })
            .ok_or_else(|| {
                cfg_err(format!(
                    "{}: base `{}` named by base_identity is not in {} — the overlay references a base this home does not have",
                    path.display(),
                    overlay.base_identity,
                    cfg_dir.join("evaluation").display()
                ))
            })?;
        let rows = store.ratification_index()?;
        let comp = compose_profile(base, &overlay, &rows)
            .map_err(|e| cfg_err(format!("{}: {e}", path.display())))?;
        let mut warnings = comp.warnings.clone();
        warnings.sort();
        // The anchor map of the human form (OQ-QH-2): only items that ACTUALLY
        // entered force, each with the decision that gave it force.
        let forces = item_forces(&overlay, &overlay_edition(&overlay), &rows);
        let ratified_in_force: BTreeMap<String, (String, String)> = comp
            .applied
            .iter()
            .filter_map(|target| {
                let (decision, actor) = forces.get(target)?.force.clone()?;
                Some((target.clone(), (decision, actor)))
            })
            .collect();
        Ok(Resolution {
            profile: Some(comp.effective),
            warnings,
            ratified: ratified_in_force,
        })
    } else {
        // No overlay for this project (REQ-SP-103).
        // 1. A profile OWNED by the task's project (`project:` equals the
        //    projects.name) keeps the pre-overlay full-profile path working
        //    — the existing devai v11 home judges devai tasks until C5
        //    migrates it (operator decision: no working profile breaks now).
        // 2. Otherwise prefer a NEUTRAL profile — one whose `project` is not
        //    the name of any registered project. A profile owned by another
        //    project must never judge this one: "задача проекта A не
        //    исполняется ни по одной проверке проекта B" cuts both ways.
        // 3. Legacy homes without a neutral profile keep today's fallback.
        let projects: Vec<String> = store.list_projects()?.into_iter().map(|p| p.name).collect();
        let auditors: Vec<&Profile> = profiles
            .iter()
            .filter(|p| p.applies_to_roles.iter().any(|r| r == "quality-auditor"))
            .collect();
        let task_project = store.get_project(pid)?.map(|p| p.name);
        let owned: Option<&Profile> = task_project
            .as_deref()
            .and_then(|name| auditors.iter().copied().find(|p| p.project == name));
        let neutral: Option<&Profile> = auditors
            .iter()
            .copied()
            .find(|p| !projects.contains(&p.project));
        let chosen = owned.or(neutral).or(fallback).cloned();
        Ok(Resolution {
            profile: chosen,
            warnings: Vec::new(),
            ratified: BTreeMap::new(),
        })
    }
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
// Agent-failure visibility (task 01a1b60c, REQ-AF-05/06/10) — platform
// protocol data.
// ---------------------------------------------------------------------------

/// Declarative policy of agent-failure visibility: the parking threshold, the
/// anti-spam window and the bounded tails. Lives in
/// `config/workflow/agent-failure.yaml` (the `agent_failure:` section); the
/// factory `Default` is that file's meaning verbatim, so a home without the
/// file behaves like one that has it at defaults (the `ResponseFormPolicy`
/// idiom).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct AgentFailurePolicy {
    /// N consecutive failures of one agent on one task park the task
    /// (REQ-AF-05 AC1). `< 1` is rejected loudly.
    pub park_threshold: u32,
    /// Anti-spam window in minutes per (agent, class) (REQ-AF-06 AC1).
    /// Zero is rejected loudly — an empty window would suppress forever.
    pub window_minutes: u32,
    /// Bounded error tail in characters carried by the event and compact
    /// surfaces (REQ-AF-01 AC4).
    pub tail_chars: usize,
    /// Bounded driver-journal tail in characters (REQ-AF-10 AC4).
    pub driver_log_chars: usize,
}

impl Default for AgentFailurePolicy {
    fn default() -> Self {
        AgentFailurePolicy {
            park_threshold: 3,
            window_minutes: 10,
            tail_chars: 2000,
            driver_log_chars: 2000,
        }
    }
}

/// Merge-scan `workflow/*.yaml` for the `agent_failure:` key with the loud
/// validation REQ-AF-05/06 demand: a threshold below 1 or a zero window is
/// an error naming the field and its meaning, never a silently ignored typo.
pub fn load_agent_failure_policy(dir: &Path) -> StoreResult<AgentFailurePolicy> {
    let mut policy = AgentFailurePolicy::default();
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
        if let Some(section) = doc.get("agent_failure") {
            policy = serde_yaml::from_value::<AgentFailurePolicy>(section.clone()).map_err(|e| {
                cfg_err(format!(
                    "{}: agent_failure must declare park_threshold (>= 1), window_minutes (>= 1) and the positive tail sizes tail_chars/driver_log_chars: {e}",
                    p.display()
                ))
            })?;
        }
    }
    if policy.park_threshold < 1 {
        return Err(cfg_err(format!(
            "{}: agent_failure.park_threshold must be >= 1 (N consecutive failures park the task)",
            dir.display()
        )));
    }
    if policy.window_minutes < 1 {
        return Err(cfg_err(format!(
            "{}: agent_failure.window_minutes must be >= 1 (the anti-spam window cannot be empty)",
            dir.display()
        )));
    }
    if policy.tail_chars < 1 || policy.driver_log_chars < 1 {
        return Err(cfg_err(format!(
            "{}: agent_failure.tail_chars and driver_log_chars must be >= 1",
            dir.display()
        )));
    }
    Ok(policy)
}

// ---------------------------------------------------------------------------
// Why a task stands still — the human words of a stop (task 01a1ca39,
// REQ-PK-03/06) — platform protocol data.
// ---------------------------------------------------------------------------

/// One declared human pair of a stop: WHY it stands and WHAT to do next.
/// Both are non-empty by validation — an empty phrase would print a blank
/// instruction where the operator expects the way out.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct ParkWords {
    pub why: String,
    pub action: String,
}

/// Declarative dictionary of the STOP surface (task 01a1ca39, REQ-PK-03):
/// one phrase + one proposed action per brake kind, per failure class and
/// per manual hold, plus the human spelling of the recording actor. Lives in
/// `config/workflow/park-reasons.yaml` (the `park_reasons:` section) — the
/// ONLY source of these words, so editing the file changes the printed
/// `task status` without a rebuild. The status read resolves every token
/// through [`ParkReasons::stop_words`] / [`ParkReasons::class_words`], and a
/// token missing from the map is a loud config error naming it, never a
/// machine name silently printed in the phrase's place.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ParkReasons {
    /// Kind token (`rounds_without_shift`, `agent_failure`, `manual_block`, …)
    /// → words.
    pub brakes: BTreeMap<String, ParkWords>,
    /// Failure class token (`spawn`, `platform`, `provider_model`, …) → words.
    pub failure_classes: BTreeMap<String, ParkWords>,
    /// Actor token → human spelling (only `platform` is a platform word; a
    /// human's own name is printed verbatim).
    pub actors: BTreeMap<String, String>,
}

impl ParkReasons {
    /// The factory bytes — the embedded `config/workflow/park-reasons.yaml`,
    /// parsed ONCE. ONE source for the default (the `Process::factory`
    /// idiom): a home without the file and a home with the untouched factory
    /// file word every stop identically.
    pub fn factory() -> &'static ParkReasons {
        static FACTORY: std::sync::OnceLock<ParkReasons> = std::sync::OnceLock::new();
        FACTORY.get_or_init(|| {
            // invariant: unreachable panic — `FACTORY_PARK_REASONS` is
            // compiled in and the loader tests parse these very bytes; an
            // invalid embedded declaration is a broken build, not a runtime
            // state.
            parse_park_reasons(
                FACTORY_PARK_REASONS,
                "embedded config/workflow/park-reasons.yaml",
            )
            .unwrap_or_else(|e| panic!("the embedded park-reasons declaration is invalid: {e}"))
        })
    }

    /// The words of a stop KIND (`rounds_without_shift`, `agent_failure`, …).
    pub fn stop_words(&self, kind: &str) -> Option<&ParkWords> {
        self.brakes.get(kind)
    }

    /// The words of a FAILURE class (`spawn`, `provider_model`, …).
    pub fn class_words(&self, class: &str) -> Option<&ParkWords> {
        self.failure_classes.get(class)
    }

    /// The actor as the operator reads it: a declared word (the platform),
    /// else the actor's own name — never rewritten.
    pub fn actor_word(&self, actor: &str) -> String {
        self.actors
            .get(actor)
            .cloned()
            .unwrap_or_else(|| actor.to_string())
    }
}

impl Default for ParkReasons {
    fn default() -> Self {
        Self::factory().clone()
    }
}

const FACTORY_PARK_REASONS: &str = include_str!("../../../config/workflow/park-reasons.yaml");

fn cfg_err_park(where_: &str, msg: impl std::fmt::Display) -> StoreError {
    cfg_err(format!("{where_}: {msg}"))
}

/// The failure classes a status response can carry: the launch classes the
/// brakes record and the agent-failure classes of the recorded failures.
fn required_failure_classes() -> Vec<&'static str> {
    let mut classes = vec![
        devai_core::types::failure_class::SPAWN,
        devai_core::types::failure_class::PLATFORM,
    ];
    classes.extend(devai_core::agent_failure::class::ALL);
    classes
}

/// Parse and validate the `park_reasons:` section. LOUD by contract
/// (REQ-PK-03 AC3/AC4): every stop kind and every failure class a response
/// can carry must be declared, no unknown key may sit silently (a typo would
/// otherwise be an inert row), and no phrase may be empty.
fn parse_park_reasons(text: &str, where_: &str) -> StoreResult<ParkReasons> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| cfg_err_park(where_, format!("parse: {e}")))?;
    let section = doc.get("park_reasons").ok_or_else(|| {
        cfg_err_park(
            where_,
            "no `park_reasons:` section — the stop surface has no words",
        )
    })?;
    // deny_unknown_fields: a typo'd key names itself here.
    let reasons: ParkReasons = serde_yaml::from_value(section.clone()).map_err(|e| {
        cfg_err_park(
            where_,
            format!("park_reasons section is invalid (keys: brakes, failure_classes, actors): {e}"),
        )
    })?;
    for kind in devai_core::types::stop_kind::ALL {
        if !reasons.brakes.contains_key(*kind) {
            return Err(cfg_err_park(
                where_,
                format!(
                    "park_reasons.brakes has no phrase for stop kind `{kind}` — declare it under brakes: (a machine name is never printed in its place)"
                ),
            ));
        }
    }
    for class in required_failure_classes() {
        if !reasons.failure_classes.contains_key(class) {
            return Err(cfg_err_park(
                where_,
                format!(
                    "park_reasons.failure_classes has no phrase for failure class `{class}` — declare it under failure_classes:"
                ),
            ));
        }
    }
    for (kind, words) in &reasons.brakes {
        if !devai_core::types::stop_kind::ALL.contains(&kind.as_str()) {
            return Err(cfg_err_park(
                where_,
                format!(
                    "park_reasons.brakes carries an unknown stop kind `{kind}` — the closed vocabulary is {:?}",
                    devai_core::types::stop_kind::ALL
                ),
            ));
        }
        if words.why.trim().is_empty() || words.action.trim().is_empty() {
            return Err(cfg_err_park(
                where_,
                format!(
                    "park_reasons.brakes.{kind} must declare a non-empty `why` and `action` — a blank phrase is not a reason"
                ),
            ));
        }
    }
    let known_classes = required_failure_classes();
    for (class, words) in &reasons.failure_classes {
        if !known_classes.contains(&class.as_str()) {
            return Err(cfg_err_park(
                where_,
                format!(
                    "park_reasons.failure_classes carries an unknown class `{class}` — the closed vocabulary is {known_classes:?}"
                ),
            ));
        }
        if words.why.trim().is_empty() || words.action.trim().is_empty() {
            return Err(cfg_err_park(
                where_,
                format!(
                    "park_reasons.failure_classes.{class} must declare a non-empty `why` and `action`"
                ),
            ));
        }
    }
    match reasons.actors.get("platform") {
        Some(word) if !word.trim().is_empty() => {}
        _ => {
            return Err(cfg_err_park(
                where_,
                "park_reasons.actors.platform must declare the platform's human spelling (`платформа`) — the raw token is not printed as a person",
            ));
        }
    }
    Ok(reasons)
}

/// Merge-scan `workflow/*.yaml` for the `park_reasons:` key, exactly the
/// `load_humanize_policy` idiom: files without the section are inert, the
/// LAST declared section wins, a malformed or incomplete declaration is a
/// LOUD config error naming the file and the missing value. An absent
/// directory/file returns the factory bytes' meaning.
pub fn load_park_reasons(dir: &Path) -> StoreResult<ParkReasons> {
    let mut policy = ParkReasons::default();
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
        if doc.get("park_reasons").is_some() {
            policy = parse_park_reasons(&text, &p.display().to_string())?;
        }
    }
    Ok(policy)
}

// ---------------------------------------------------------------------------
// Humanization template of role artifacts (task 01a1acfe, REQ-HP-04) —
// platform protocol data.
// ---------------------------------------------------------------------------

/// One declared explanation slot of the human paragraph (REQ-HP-03: the
/// paragraph is not «empty chatter» only when every slot is filled — a slot
/// is filled when ANY of its marker phrases appears in the paragraph,
/// case-insensitive substring, deterministic).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct HumanizeSlot {
    pub name: String,
    pub markers: Vec<String>,
}

/// Declarative humanization template of the role-borne artifacts (REQ-HP-04):
/// register, language, min/max volume, required explanation structure and
/// the edibility-detector thresholds live in `config/workflow/humanize.yaml`
/// (the `humanize:` section) — project-tunable data, never code literals.
/// The mechanical detector (`devai_runtime::humanize`) reads this policy on
/// every publication path; the factory `Default` is the embedded file's
/// meaning verbatim, so a home without the file behaves exactly like one
/// that has it at defaults (the `ResponseFormPolicy` idiom).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct HumanizePolicy {
    /// Ratified OQ-HP-6: the field name fixed by the spec — declared as a
    /// required top-level field in the role-borne contract schemas; the
    /// detector fires only on contracts whose schema requires this field.
    pub field: String,
    /// Ratified OQ-HP-3: fallback language for statements with no expressed
    /// language (the paragraph otherwise follows the business description).
    /// Feeds the prompt template — the gate is language-independent.
    pub language: String,
    pub min_chars: usize,
    pub max_chars: usize,
    /// «Restatement of formalism» threshold: the fraction of the paragraph's
    /// word n-grams already present in the artifact's formal body.
    pub overlap_max_ratio: f64,
    pub ngram: usize,
    /// Body fields excluded from the overlap corpus (human echo prose is
    /// not the formal structure the paragraph must not restate).
    pub overlap_ignore: Vec<String>,
    /// Register: literal markers of the formal protocol a plain-words
    /// paragraph must not carry (case-insensitive substrings).
    pub forbidden_markers: Vec<String>,
    /// Required explanation structure (what happens / what will change).
    pub slots: Vec<HumanizeSlot>,
}

impl Default for HumanizePolicy {
    /// The factory bytes (`config/workflow/humanize.yaml`) are the fallback:
    /// presence, min/max are hard, overlap fires only on blatant
    /// copy-retelling — the conservative calibration stance of the review.
    fn default() -> Self {
        HumanizePolicy {
            field: default_humanize_field(),
            language: default_humanize_language(),
            min_chars: default_humanize_min(),
            max_chars: default_humanize_max(),
            overlap_max_ratio: default_humanize_overlap(),
            ngram: default_humanize_ngram(),
            overlap_ignore: vec!["business_description".into()],
            forbidden_markers: vec![
                "REQ-".into(),
                "OQ-".into(),
                "ASM-".into(),
                "acceptance_criteria".into(),
            ],
            slots: default_humanize_slots(),
        }
    }
}

fn default_humanize_field() -> String {
    "human_paragraph".into()
}
fn default_humanize_language() -> String {
    "ru".into()
}
fn default_humanize_min() -> usize {
    200
}
fn default_humanize_max() -> usize {
    1200
}
fn default_humanize_overlap() -> f64 {
    0.6
}
fn default_humanize_ngram() -> usize {
    3
}
fn default_humanize_slots() -> Vec<HumanizeSlot> {
    vec![
        HumanizeSlot {
            name: "что происходит".into(),
            markers: [
                "сейчас",
                "сегодня",
                "раньше",
                "ранее",
                "до сих пор",
                "происходит",
                "работает",
                "работало",
                "было",
                "now",
                "currently",
                "today",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        },
        HumanizeSlot {
            name: "что изменится".into(),
            markers: [
                "изменится",
                "изменят",
                "изменит",
                "будет",
                "станут",
                "станет",
                "добавит",
                "появится",
                "теперь",
                "начнёт",
                "начнут",
                "чинит",
                "заменит",
                "после",
                "will",
                "becomes",
                "adds",
                "now checks",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        },
    ]
}

impl HumanizePolicy {
    /// Bounds are checked LOUD at load: an inverted min/max or a ratio
    /// outside (0,1] would silently reject (or never reject) every
    /// paragraph — the operator gets the file, the key and the meaning.
    fn validate(&self, where_: &str) -> StoreResult<()> {
        if self.field.trim().is_empty() {
            return Err(cfg_err(format!(
                "{where_}: humanize.field must name the paragraph field (ratified OQ-HP-6: `human_paragraph`)"
            )));
        }
        if self.min_chars > self.max_chars {
            return Err(cfg_err(format!(
                "{}: humanize.min_chars ({}) must not exceed humanize.max_chars ({}) — the volume window is empty",
                where_, self.min_chars, self.max_chars
            )));
        }
        if !(self.overlap_max_ratio > 0.0 && self.overlap_max_ratio <= 1.0) {
            return Err(cfg_err(format!(
                "{}: humanize.overlap_max_ratio must be in (0, 1] (got {}) — the fraction of paragraph n-grams allowed to repeat the formal body",
                where_, self.overlap_max_ratio
            )));
        }
        if self.ngram == 0 {
            return Err(cfg_err(format!(
                "{where_}: humanize.ngram must be >= 1 (word n-gram size of the overlap detector)"
            )));
        }
        for s in &self.slots {
            if s.name.trim().is_empty() {
                return Err(cfg_err(format!(
                    "{where_}: humanize.slots entry has an empty `name` — a slot must be declared to be checked"
                )));
            }
        }
        Ok(())
    }
}

/// Merge-scan `workflow/*.yaml` for the `humanize:` key, exactly the
/// `load_response_form_policy` idiom: files without the section are inert,
/// the LAST declared section wins, a malformed declaration or a typo'd key
/// is a LOUD config error naming the file (REQ-HP-04 AC2 — never a
/// default-silence). An absent directory/file returns the factory defaults.
pub fn load_humanize_policy(dir: &Path) -> StoreResult<HumanizePolicy> {
    let mut policy = HumanizePolicy::default();
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
        if let Some(section) = doc.get("humanize") {
            // deny_unknown_fields: a typo'd key names itself here.
            policy = serde_yaml::from_value::<HumanizePolicy>(section.clone()).map_err(|e| {
                cfg_err(format!(
                    "{}: humanize section is invalid (keys: field, language, min_chars, max_chars, overlap_max_ratio, ngram, overlap_ignore, forbidden_markers, slots): {e}",
                    p.display()
                ))
            })?;
            policy.validate(&p.display().to_string())?;
        }
    }
    Ok(policy)
}

// ---------------------------------------------------------------------------
// Human task name template (task 01a1ad1d, REQ-TN-05) — platform protocol
// data.
// ---------------------------------------------------------------------------

/// Declarative discipline of the human task name (REQ-TN-05): the field
/// name, the volume window and the one-line rule live in
/// `config/workflow/task-name.yaml` (the `task_name:` section) —
/// project-tunable data, never code literals ("Запечённые в код числа
/// запрещены"). The mechanical detector (`devai_runtime::taskname`) reads
/// this policy on every publication path AND the manual `task.rename` gate
/// applies the SAME window; the factory `Default` is the embedded file's
/// meaning verbatim, so a home without the file behaves exactly like one
/// that has it at defaults (the `HumanizePolicy` idiom).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct TaskNamePolicy {
    /// The field of the spec artifact that carries the name — fixed by the
    /// contract schema, declared here so producer, gate and reader speak
    /// one literal (the ratified OQ-HP-6 habit, REQ-TN-02 constraint).
    pub field: String,
    pub min_chars: usize,
    pub max_chars: usize,
    /// A legal name fits ONE list/graph line: no embedded newlines.
    pub single_line: bool,
}

impl Default for TaskNamePolicy {
    /// The factory bytes (`config/workflow/task-name.yaml`) — a name long
    /// enough to say what the task is, short enough to sign a row, and
    /// always a single line.
    fn default() -> Self {
        TaskNamePolicy {
            field: default_task_name_field(),
            min_chars: default_task_name_min(),
            max_chars: default_task_name_max(),
            single_line: true,
        }
    }
}

fn default_task_name_field() -> String {
    "task_name".into()
}
fn default_task_name_min() -> usize {
    3
}
fn default_task_name_max() -> usize {
    60
}

impl TaskNamePolicy {
    /// Bounds checked LOUD at load: a blank field name or an inverted
    /// window would silently reject (or never reject) every name — the
    /// operator gets the file, the key and the meaning (REQ-TN-05: an
    /// out-of-range value is a loud config error, never a swallowed
    /// default).
    fn validate(&self, where_: &str) -> StoreResult<()> {
        if self.field.trim().is_empty() {
            return Err(cfg_err(format!(
                "{where_}: task_name.field must name the spec field carrying the human name (factory value: `task_name`)"
            )));
        }
        if self.min_chars > self.max_chars {
            return Err(cfg_err(format!(
                "{}: task_name.min_chars ({}) must not exceed task_name.max_chars ({}) — the volume window is empty",
                where_, self.min_chars, self.max_chars
            )));
        }
        if self.min_chars == 0 {
            return Err(cfg_err(format!(
                "{where_}: task_name.min_chars must be >= 1 — a zero floor admits the empty name (the window rejects blank by construction, so a 0 here is a typo)"
            )));
        }
        Ok(())
    }
}

/// Merge-scan `workflow/*.yaml` for the `task_name:` key, exactly the
/// `load_humanize_policy` idiom: files without the section are inert, the
/// LAST declared section wins, a malformed declaration or a typo'd key is a
/// LOUD config error naming the file — never a default-silence. An absent
/// directory/file returns the factory defaults.
pub fn load_task_name_policy(dir: &Path) -> StoreResult<TaskNamePolicy> {
    let mut policy = TaskNamePolicy::default();
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
        if let Some(section) = doc.get("task_name") {
            // deny_unknown_fields: a typo'd key names itself here.
            policy = serde_yaml::from_value::<TaskNamePolicy>(section.clone()).map_err(|e| {
                cfg_err(format!(
                    "{}: task_name section is invalid (keys: field, min_chars, max_chars, single_line): {e}",
                    p.display()
                ))
            })?;
            policy.validate(&p.display().to_string())?;
        }
    }
    Ok(policy)
}

// ---------------------------------------------------------------------------
// PROCESS DECLARATION (REQ-AR-09/11/14/15/16) — the state machine as data.
// ---------------------------------------------------------------------------
//
// The DECLARATION itself (states, named conditions, transitions, holds,
// brakes, validation) lives in `devai_core::process`: the workflow commands
// resolve every target there, so the domain owns the data AND the resolution.
// This module owns only the FILE: the directory scan, the `where_` labelling
// and the mapping onto the store's config error type.

pub use devai_core::process::{
    HoldKind, PROCESS_RETURN_STATE, Process, ProcessBrakes, ProcessHolds, ProcessState,
    ProcessTransition, SUPPORTED_PROCESS_VERSION, Target, process_condition, process_executor,
};

/// The declaration under its historical name — one type, two spellings that
/// both mean `devai_core::process::Process`.
pub use devai_core::process::Process as ProcessPolicy;

/// The factory declaration — the embedded `config/workflow/process.yaml`.
/// ONE source for the default: the loader parses these very bytes, so a home
/// without the file and a home with the untouched factory file behave
/// identically (the `factory_inbox_wait_policy` idiom).
pub fn factory_process_policy() -> StoreResult<Process> {
    Ok(Process::factory().clone())
}

/// Parse one declaration document and validate it. A malformed or diverging
/// declaration is a LOUD config error naming the file — never a default.
fn parse_process_policy(text: &str, where_: &str) -> StoreResult<Process> {
    devai_core::process::parse(text, where_).map_err(cfg_err)
}

/// Merge-scan `workflow/*.yaml` for the `process:` key — the same idiom as
/// `load_humanize_policy`: files without the section are inert, the LAST
/// declared section wins, a malformed declaration is a LOUD config error
/// naming the file. An absent directory/file returns the factory bytes.
pub fn load_process_policy(dir: &Path) -> StoreResult<Process> {
    let mut policy: Option<Process> = None;
    if dir.is_dir() {
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
            if doc.get("process").is_some() {
                policy = Some(parse_process_policy(&text, &p.display().to_string())?);
            }
        }
    }
    match policy {
        Some(p) => Ok(p),
        None => factory_process_policy(),
    }
}

// REWORK CONTEXT delivery policy (R-001..R-007) — platform workflow data.
// ---------------------------------------------------------------------------

/// The factory table — the embedded `config/workflow/inbox-waits.yaml`.
/// ONE source for the default: the loader parses these very bytes, so a
/// home without the file and a home with the untouched factory file render
/// exactly the same section (no second Rust copy that could drift).
pub fn factory_inbox_wait_policy() -> StoreResult<InboxWaitPolicy> {
    parse_inbox_wait_policy(
        FACTORY_INBOX_WAITS,
        "embedded config/workflow/inbox-waits.yaml",
    )
}

const FACTORY_INBOX_WAITS: &str = include_str!("../../../config/workflow/inbox-waits.yaml");

fn parse_inbox_wait_policy(text: &str, where_: &str) -> StoreResult<InboxWaitPolicy> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| cfg_err(format!("parse {where_}: {e}")))?;
    let section = doc.get("inbox_waits").ok_or_else(|| {
        cfg_err(format!(
            "{where_}: no `inbox_waits:` section — the wait section has no demand table"
        ))
    })?;
    let policy = serde_yaml::from_value::<InboxWaitPolicy>(section.clone())
        .map_err(|e| cfg_err(format!("{where_}: inbox_waits section is invalid: {e}")))?;
    validate_inbox_wait_policy(&policy, where_)?;
    Ok(policy)
}

/// Vocabulary and completeness validation, LOUD by contract:
/// * every `TaskState` variant has a row (a new variant must get a membership
///   decision, not silently drop out of the section);
/// * no unknown state token (a typo would otherwise be an inert row);
/// * a wait state carries a demand and at least one CLI command, either
///   directly or through variants; a non-wait state carries neither (so the
///   table cannot claim a demand for a state that is not shown);
/// * every `when` predicate is one the renderer knows;
/// * every command template mentions an existing placeholder or none.
fn validate_inbox_wait_policy(policy: &InboxWaitPolicy, where_: &str) -> StoreResult<()> {
    for state in devai_core::types::TaskState::ALL {
        if !policy.states.contains_key(state.as_str()) {
            return Err(cfg_err(format!(
                "{where_}: inbox_waits.states has no row for `{}` — every TaskState needs a group decision",
                state.as_str()
            )));
        }
    }
    for (token, row) in &policy.states {
        devai_core::types::TaskState::parse(token).map_err(|_| {
            cfg_err(format!(
                "{where_}: inbox_waits.states.`{token}` is not a TaskState token (accepted: {})",
                devai_core::types::TaskState::ALL
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        validate_wait_row(token, row, where_)?;
    }
    Ok(())
}

/// One state's row: the group/demand consistency, its variants and its
/// command templates.
fn validate_wait_row(
    token: &str,
    row: &devai_core::inbox::WaitStatePolicy,
    where_: &str,
) -> StoreResult<()> {
    let demand = row.demand.trim();
    let commands: Vec<&str> = row.commands.iter().map(|c| c.trim()).collect();
    match row.group {
        devai_core::inbox::WaitGroup::NotWait => {
            if !demand.is_empty() || !commands.is_empty() || !row.variants.is_empty() {
                return Err(cfg_err(format!(
                    "{where_}: `{token}` has group `none` but declares a demand/command/variant — a non-wait state is never shown"
                )));
            }
        }
        _ => {
            if row.variants.is_empty()
                && (demand.is_empty() || commands.iter().all(|c| c.is_empty()))
            {
                return Err(cfg_err(format!(
                    "{where_}: `{token}` is a wait state but has no demand or no CLI command"
                )));
            }
        }
    }
    for variant in &row.variants {
        validate_wait_variant(token, variant, where_)?;
    }
    let all_commands = row
        .commands
        .iter()
        .chain(row.variants.iter().flat_map(|v| v.commands.iter()));
    for command in all_commands {
        validate_wait_command(token, command, where_)?;
    }
    Ok(())
}

/// One conditional wording: a KNOWN predicate and a non-empty demand plus at
/// least one command.
fn validate_wait_variant(
    token: &str,
    variant: &devai_core::inbox::WaitVariant,
    where_: &str,
) -> StoreResult<()> {
    if !devai_core::inbox::WHEN_ALL.contains(&variant.when.as_str()) {
        return Err(cfg_err(format!(
            "{where_}: `{token}` variant `when: {}` is unknown (accepted: {})",
            variant.when,
            devai_core::inbox::WHEN_ALL.join(", ")
        )));
    }
    if variant.demand.trim().is_empty() || variant.commands.iter().all(|c| c.trim().is_empty()) {
        return Err(cfg_err(format!(
            "{where_}: `{token}` variant `when: {}` has no demand or no CLI command",
            variant.when
        )));
    }
    Ok(())
}

/// A placeholder-carrying template must actually be a CLI invocation, so the
/// printed hint stays runnable as printed.
fn validate_wait_command(token: &str, command: &str, where_: &str) -> StoreResult<()> {
    for placeholder in ["{id}", "{question_id}"] {
        if command.contains(placeholder) && !command.contains("devai ") {
            return Err(cfg_err(format!(
                "{where_}: `{token}` command `{command}` carries a placeholder but is not a CLI template"
            )));
        }
    }
    Ok(())
}

/// Merge-scan `workflow/*.yaml` for the `inbox_waits:` key with the
/// `load_humanize_policy` idiom: files without the section are inert, the
/// LAST declared section wins, a malformed table is a LOUD config error
/// naming the file. An absent directory/file yields the factory table
/// parsed from the embedded bytes (never an empty section).
pub fn load_inbox_wait_policy(dir: &Path) -> StoreResult<InboxWaitPolicy> {
    let mut policy = factory_inbox_wait_policy()?;
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
        if doc.get("inbox_waits").is_some() {
            policy = parse_inbox_wait_policy(&text, &p.display().to_string())?;
        }
    }
    Ok(policy)
}

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

    /// REQ-SP-101/102 (cycle C1): the factory ships the ABSTRACT BASE only —
    /// one profile, no project specifics anywhere, and it compiles into the
    /// auditor exactly like any full Profile.
    #[test]
    fn factory_profile_loads_valid_and_compiles_into_auditor() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(dir.path()).unwrap();
        let cfg = load(dir.path()).unwrap();
        // The factory profile passes grammar validation on load.
        assert_eq!(cfg.profiles.len(), 1);
        let p = &cfg.profiles[0];
        assert_eq!(p.project, "base");
        assert_eq!(p.identity(), "base@1");
        assert!(
            p.applies_to_roles.iter().any(|r| r == "quality-auditor"),
            "the engine must find the base for the auditor"
        );
        // Механических гейтов база не несёт: гейт — это команда, а любая команда
        // есть знание о конкретном проекте (пути, инструменты, имена файлов).
        // Проверки остаются все, но судятся ролью по абстрактным критериям.
        assert!(p.gates.is_empty(), "the base carries no mechanical gates");
        assert!(p.checks().iter().any(|c| c.id == "cl/meaningful-names"));
        assert!(p.checks().iter().any(|c| c.id == "pr/spec-fidelity"));
        // AC REQ-SP-101: no devai specifics at start — no block, no name.
        assert!(
            !p.evaluations.iter().any(|e| e.id == "platform-invariants"),
            "the base carries no platform-invariants block"
        );
        let text = fs::read_to_string(dir.path().join("evaluation/base.yaml")).unwrap();
        for leak in ["devai", "cargo", "crates", "rusqlite", "thiserror"] {
            assert!(
                !text.contains(leak),
                "the factory base mentions concrete-stack token `{leak}`"
            );
        }
        // Правило остаётся правилом, только пока не знает ничего о проекте:
        // ни команд, ни слотов-заглушек, ни путей. Проба («запрос проверки»)
        // — это всегда конкретика, поэтому в базе их нет вовсе.
        for c in p.checks() {
            assert!(
                c.probes.is_empty(),
                "check `{}` carries a probe; the base must stay free of commands",
                c.id
            );
        }
        // Floors are derived from error_type severity, never declared per check.
        let inj = p
            .checks()
            .into_iter()
            .find(|c| c.id == "sec/injection")
            .unwrap();
        assert_eq!(p.worst_severity(inj), Some("catastrophic"));
        assert_eq!(p.floor_for(inj), Some(0.7));
        // Auditor instructions carry the profile-compiled rubric checklist.
        let rf = cfg
            .roles
            .iter()
            .find(|r| r.slug == "quality-auditor")
            .unwrap();
        let role = cfg.build_role(rf, Utc::now()).unwrap();
        assert!(role.instructions.contains("Evaluation profile `base`"));
        assert!(
            role.instructions.contains("[mt/dry]"),
            "{}",
            role.instructions
        );
        assert!(role.instructions.contains("anchors: 1.0"));
        // Human-signed checks are named but excluded from agent scoring.
        assert!(role.instructions.contains("Human-signed checks"));
        assert!(role.instructions.contains("pr/spec-fidelity"));
        // Mechanical checks never appear in the agent's checklist.
        assert!(
            !role
                .instructions
                .contains("[cl/style-consistency] (mechanical")
        );
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
        let restored = p.scoring.weights.remove("mt/dry").expect("weight present");
        assert!(p.validate().unwrap_err().contains("V7"));
        // V1: an undeclared detected type is rejected.
        p.scoring.weights.insert("mt/dry".into(), restored);
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

    /// C1 transition precedence (REQ-SP-101 × the operator's approved
    /// decision "до C5 механизм не должен ломать работающий профиль"):
    /// re-init adds `base.yaml` to an existing home, and the auditor role
    /// must NOT silently swap its checklist from the home's owned legacy
    /// full profile to the factory base — legacy wins while such a profile
    /// exists; with base alone (a clean home) the base compiles.
    #[test]
    fn legacy_owned_profile_outlives_the_factory_base_until_c5() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        materialize_if_absent(&cfgdir).unwrap();
        let cfg = load(&cfgdir).unwrap();
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(
            cfg.profile_for("quality-auditor").unwrap().identity(),
            "base@1"
        );
        // Simulate the existing home: quality.yaml v11 alongside the new base.
        fs::write(
            cfgdir.join("evaluation/quality.yaml"),
            r#"version: 11
project: devai
applies_to_roles: [quality-auditor]
error_types:
  architecture/layer-leak: { desc: d, severity: catastrophic }
gates:
  - id: build-green
    probes: [{ cmd: "cargo build --workspace", pass_exit: [0] }]
evaluations:
  - id: platform-invariants
    title: Продуктовые инварианты
    checks:
      - id: arch/legacy-check
        statement: legacy invariant
        detects: [architecture/layer-leak]
        verify: rubric
        anchors: { "1.0": a, "0.5": b, "0.0": c }
        reviewer: human
scoring:
  theta: 0.8
  floors: { catastrophic: 0.7 }
  blocked_policy: { max_blocked_weight: 0.2 }
  weights: { arch/legacy-check: 1.0 }
"#,
        )
        .unwrap();
        let cfg = load(&cfgdir).unwrap();
        assert_eq!(cfg.profiles.len(), 2);
        let p = cfg.profile_for("quality-auditor").unwrap();
        assert_eq!(p.project, "devai");
        assert_eq!(p.version, 11);
        let rf = cfg
            .roles
            .iter()
            .find(|r| r.slug == "quality-auditor")
            .unwrap();
        let role = cfg.build_role(rf, Utc::now()).unwrap();
        assert!(
            role.instructions.contains("Evaluation profile `devai`"),
            "the legacy checklist survives re-init of an existing home"
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
        let mut latest: BTreeMap<&str, (u32, String)> = BTreeMap::new();
        for (name, version, schema) in crate::seed::contracts() {
            let e = latest.entry(name).or_insert((0, String::new()));
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
        let (_, _v2, schema) = crate::seed::contracts()
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

    /// R3: a contract version is declared ONCE, in `devai_core::types`. The
    /// role fields that name a contract point at that SAME declaration, and
    /// no role prose re-spells a `quality-report@N` literal — so a contract
    /// bump cannot leave a stale version in a role or in the compiled prompt.
    #[test]
    fn role_contract_refs_follow_the_platform_declared_versions() {
        use devai_core::types::{ASSESSMENT_CONTRACT, QUALITY_REPORT_CONTRACT};
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let cfg = load(&dir).unwrap();
        let role = |slug: &str| {
            let mut found = cfg.roles.iter().filter(|r| r.slug == slug);
            let one = found.next().unwrap_or_else(|| panic!("no role {slug}"));
            assert!(found.next().is_none(), "duplicate role {slug}");
            one
        };
        let assessment = format!("{}@{}", ASSESSMENT_CONTRACT.0, ASSESSMENT_CONTRACT.1);
        assert_eq!(
            role("quality-auditor").output_contract.as_deref(),
            Some(assessment.as_str()),
            "the auditor answers in the platform's assessment contract"
        );
        let report = format!(
            "{}@{}",
            QUALITY_REPORT_CONTRACT.0, QUALITY_REPORT_CONTRACT.1
        );
        assert_eq!(
            role("quality-aggregator").input_contract.as_deref(),
            Some(report.as_str()),
            "the aggregator consumes the published report contract"
        );
        for r in &cfg.roles {
            for (field, text) in [
                ("purpose", &r.purpose),
                ("instructions", &r.instructions),
                ("done_criteria", &r.done_criteria),
            ] {
                assert!(
                    !text.contains("quality-report@"),
                    "{field} of {} spells a contract version instead of naming the gate",
                    r.slug
                );
            }
        }
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
                .any(|p| p.file_name().is_some_and(|n| n == "base.yaml")
                    && p.parent()
                        .is_some_and(|d| d.file_name().is_some_and(|n| n == "evaluation"))),
            "re-init must bring the new factory profile into an old home: {written:?}"
        );
        assert_eq!(
            fs::read_to_string(cfgdir.join("criteria/review.yaml")).unwrap(),
            "mine"
        );
    }

    /// REQ-TN-05 AC2 (task 01a1ad1d): the name template is the humanize /
    /// response-form discipline again — factory materialisation, an edit
    /// without rebuild, an absent file behaving exactly like the factory
    /// bytes, and a broken declaration FAILING LOUD, never swallowed.
    #[test]
    fn task_name_template_is_declarative_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        materialize_if_absent(&cfgdir).unwrap();
        let wf = cfgdir.join("workflow");
        // The factory file lands in the home next to humanize.yaml.
        assert!(
            wf.join("task-name.yaml").exists(),
            "init must materialise the task-name template"
        );
        let pol = load_task_name_policy(&wf).unwrap();
        assert_eq!(pol.field, "task_name");
        assert_eq!(
            (pol.min_chars, pol.max_chars, pol.single_line),
            (3, 60, true)
        );

        // A home predating the file: the factory meaning, no error.
        fs::remove_file(wf.join("task-name.yaml")).unwrap();
        assert_eq!(
            load_task_name_policy(&wf).unwrap(),
            TaskNamePolicy::default()
        );
        fs::remove_dir_all(&wf).unwrap();
        assert_eq!(
            load_task_name_policy(&wf).unwrap(),
            TaskNamePolicy::default(),
            "absent dir is not an error"
        );

        // Operator edit — no rebuild (AC2).
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("task-name.yaml"),
            "task_name:\n  field: task_name\n  min_chars: 5\n  max_chars: 40\n  single_line: false\n",
        )
        .unwrap();
        let pol = load_task_name_policy(&wf).unwrap();
        assert_eq!(
            (pol.min_chars, pol.max_chars, pol.single_line),
            (5, 40, false)
        );

        // Inverted window: a LOUD config error naming the file (AC1: an
        // empty window would reject every name — never a silent default).
        fs::write(
            wf.join("task-name.yaml"),
            "task_name:\n  min_chars: 80\n  max_chars: 40\n",
        )
        .unwrap();
        let e = load_task_name_policy(&wf).unwrap_err().to_string();
        assert!(
            e.contains("min_chars") && e.contains("task-name.yaml"),
            "inverted window must name itself: {e}"
        );

        // Typo'd key: deny_unknown_fields names it — the humanize idiom.
        fs::write(wf.join("task-name.yaml"), "task_name:\n  max_char: 40\n").unwrap();
        let e = load_task_name_policy(&wf).unwrap_err().to_string();
        assert!(
            e.contains("task_name section is invalid"),
            "a typo'd key must be loud: {e}"
        );
    }
}

#[cfg(test)]
mod process_declaration_tests {
    use super::*;

    /// The factory declaration is the state machine the platform ships: the
    /// declared executor map must be exactly today's stage roles, and it must
    /// pass its own validation (a factory file that does not is a build bug).
    #[test]
    fn the_factory_declaration_maps_every_stage_to_its_role() {
        let p = factory_process_policy().expect("factory declaration is valid");
        assert_eq!(p.version, SUPPORTED_PROCESS_VERSION);
        for (state, role) in [
            ("ANALYSIS", "requirements-analyst"),
            ("REVIEW", "requirements-reviewer"),
            ("IMPLEMENTATION", "implementation-agent"),
            ("REWORK", "implementation-agent"),
            ("QA", "quality-auditor"),
        ] {
            assert_eq!(p.role_for(state), Some(role), "state {state}");
        }
        // A human gate and a terminal state declare NO role: nothing is
        // launched there (REQ-AR-11 AC2).
        assert_eq!(p.role_for("APPROVAL"), None);
        assert_eq!(p.executor_for("APPROVAL"), Some(process_executor::HUMAN));
        assert_eq!(p.executor_for("ACCEPTED"), Some(process_executor::NONE));
        // The brakes are data too.
        assert!(p.brakes.max_rounds_without_shift >= 1);
        assert!(p.brakes.max_consecutive_failures >= 1);
    }

    /// REQ-AR-16: a declaration from another version is a TYPED refusal that
    /// names both versions — never a silent run under unknown rules.
    #[test]
    fn a_foreign_declaration_version_is_refused_with_both_versions_named() {
        let mut p = factory_process_policy().unwrap();
        p.version = SUPPORTED_PROCESS_VERSION + 1;
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains("process.version"), "{e}");
        assert!(
            e.contains(&(SUPPORTED_PROCESS_VERSION + 1).to_string()),
            "{e}"
        );
        assert!(e.contains(&SUPPORTED_PROCESS_VERSION.to_string()), "{e}");
    }

    /// REQ-AR-09: the declaration names the failure CLASSES that park at
    /// once. A name outside the recorded vocabulary would silently never
    /// fire — an inert brake is worse than none — so init refuses it.
    #[test]
    fn a_park_immediately_class_outside_the_vocabulary_is_refused() {
        let mut p = factory_process_policy().unwrap();
        assert!(p.parks_immediately(devai_core::types::failure_class::PLATFORM));
        // `spawn` is deliberately NOT immediate: the consecutive counter owns
        // a transient spawn failure, so one attempt never parks the task.
        assert!(!p.parks_immediately(devai_core::types::failure_class::SPAWN));
        p.brakes.park_immediately.push("no_such_class".into());
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains("no_such_class"), "{e}");
        assert!(e.contains("park_immediately"), "{e}");
    }

    /// REQ-AR-14: an outcome with no transition is a DEAD END and init
    /// refuses it, naming the state and the uncovered outcome.
    #[test]
    fn an_uncovered_outcome_refuses_the_declaration_as_a_dead_end() {
        let mut p = factory_process_policy().unwrap();
        p.transitions
            .retain(|t| !(t.from == "ANALYSIS" && t.on == process_condition::OUTCOME_MISSING));
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains("DEAD END"), "{e}");
        assert!(e.contains("ANALYSIS"), "{e}");
        assert!(e.contains(process_condition::OUTCOME_MISSING), "{e}");
    }

    /// REQ-AR-15: conditions are NAMES from the closed list — an unknown name
    /// (a typo, an expression) is refused by name.
    #[test]
    fn a_transition_on_an_unknown_condition_name_is_refused() {
        let mut p = factory_process_policy().unwrap();
        // An extra transition whose condition is an EXPRESSION rather than a
        // declared name — exactly what the closed list refuses.
        p.transitions.push(ProcessTransition {
            from: "ANALYSIS".into(),
            on: "role.finished == true".into(),
            to: "REVIEW".into(),
        });
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains("role.finished == true"), "{e}");
        assert!(e.contains("condition name"), "{e}");
    }

    /// A role state must cover the three platform-emitted outcomes — dropping
    /// one from `outcomes` is refused even when a transition exists.
    #[test]
    fn a_role_state_must_declare_its_launch_and_missing_outcomes() {
        let mut p = factory_process_policy().unwrap();
        for st in &mut p.states {
            if st.state == devai_core::types::TaskState::Rework.as_str() {
                st.outcomes
                    .retain(|o| o != process_condition::LAUNCH_FAILED);
            }
        }
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains("REWORK"), "{e}");
        assert!(e.contains(process_condition::LAUNCH_FAILED), "{e}");
    }

    /// REQ-AR-15 / rework R10: conditions are a CLOSED vocabulary on BOTH
    /// sides. A name the declaration invents is refused by init instead of
    /// living on as an edge no command can ever emit.
    #[test]
    fn a_declared_condition_this_build_never_emits_is_refused_by_init() {
        let mut p = factory_process_policy().unwrap();
        // The factory file covers the build's vocabulary exactly.
        assert_eq!(p.conditions.len(), process_condition::ALL.len());
        p.conditions.push("role_finished".into());
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains("role_finished"), "{e}");
        assert!(e.contains("never emits"), "{e}");

        // And the other direction is refused too: a build-emitted name the
        // declaration dropped can never be expressed by a transition.
        let mut p = factory_process_policy().unwrap();
        p.conditions
            .retain(|c| c != process_condition::OUTCOME_MISSING);
        let e = p.validate("process.yaml").unwrap_err().to_string();
        assert!(e.contains(process_condition::OUTCOME_MISSING), "{e}");
    }

    /// A declaration loaded from the home's workflow directory is the one in
    /// force (data changes behavior, not code) — and a broken one fails the
    /// load loudly.
    #[test]
    fn the_loader_reads_the_home_declaration_and_refuses_a_broken_one() {
        let d = tempfile::TempDir::new().unwrap();
        let wf = d.path().join("workflow");
        fs::create_dir_all(&wf).unwrap();
        let factory = include_str!("../../../config/workflow/process.yaml");
        fs::write(wf.join("process.yaml"), factory).unwrap();
        let p = load_process_policy(&wf).unwrap();
        assert_eq!(p.role_for("QA"), Some("quality-auditor"));

        // No `process:` section anywhere -> factory bytes (the humanize idiom).
        fs::write(wf.join("process.yaml"), "humanize:\n  min_chars: 1\n").unwrap();
        let p = load_process_policy(&wf).unwrap();
        assert_eq!(p.version, SUPPORTED_PROCESS_VERSION);

        // A broken declaration is loud and names the file.
        fs::write(wf.join("process.yaml"), "process:\n  version: 1\n").unwrap();
        let e = load_process_policy(&wf).unwrap_err().to_string();
        assert!(e.contains("process.yaml"), "{e}");
    }

    /// REQ-PK-03: the words of a stop are DECLARATION data. The factory file
    /// materialises, an operator edit changes the phrase with no rebuild, a
    /// home without the file words every stop from the same factory bytes, and
    /// a missing or typo'd value is a LOUD error naming it — never a silent
    /// machine-token fallback (REQ-PK-03 AC1/AC2/AC3/AC4).
    #[test]
    fn park_reasons_are_declarative_and_validated_loudly() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfgdir = dir.path().join("config");
        materialize_if_absent(&cfgdir).unwrap();
        let wf = cfgdir.join("workflow");
        let reasons = load_park_reasons(&wf).unwrap();
        assert_eq!(reasons.actor_word("platform"), "платформа");
        assert_eq!(
            reasons.actor_word("guerrero"),
            "guerrero",
            "a human's own name is never rewritten"
        );
        // Every stop kind and every failure class a response can carry has
        // words — the factory file is complete by construction.
        for kind in devai_core::types::stop_kind::ALL {
            let w = reasons.stop_words(kind).expect(kind);
            assert!(!w.why.trim().is_empty() && !w.action.trim().is_empty());
        }
        for class in devai_core::agent_failure::class::ALL {
            let w = reasons.class_words(class).expect(class);
            assert!(!w.why.trim().is_empty() && !w.action.trim().is_empty());
        }

        // Absent directory: the factory MEANING (one source of the default).
        fs::remove_dir_all(&wf).unwrap();
        assert_eq!(load_park_reasons(&wf).unwrap(), *ParkReasons::factory());

        // Operator edit — no rebuild, no code change (REQ-PK-03 AC2).
        fs::create_dir_all(&wf).unwrap();
        let factory = include_str!("../../../config/workflow/park-reasons.yaml");
        let edited = factory.replace(
            "сработал тормоз кругов",
            "круги кончились без нового артефакта",
        );
        assert_ne!(factory, edited);
        fs::write(wf.join("park-reasons.yaml"), &edited).unwrap();
        assert!(
            load_park_reasons(&wf)
                .unwrap()
                .stop_words("rounds_without_shift")
                .unwrap()
                .why
                .contains("круги кончились")
        );

        // A typo'd key: the required phrase disappears and the error names it.
        let typo = factory.replace("    manual_pause:", "    manual_pauze:");
        assert_ne!(factory, typo);
        fs::write(wf.join("park-reasons.yaml"), &typo).unwrap();
        let e = load_park_reasons(&wf).unwrap_err().to_string();
        assert!(e.contains("manual_pause"), "{e}");

        // An unknown failure class is rejected by name as well.
        let unknown = factory.replace(
            "    unclassified:\n",
            "    typo_class:\n      why: \"x\"\n      action: \"y\"\n    unclassified:\n",
        );
        assert_ne!(factory, unknown);
        fs::write(wf.join("park-reasons.yaml"), &unknown).unwrap();
        let e = load_park_reasons(&wf).unwrap_err().to_string();
        assert!(e.contains("typo_class"), "{e}");

        // A missing actors.platform is loud too — the raw token is not a person.
        let no_actor = factory.replace("  actors:\n    platform: \"платформа\"\n", "");
        assert_ne!(factory, no_actor);
        fs::write(wf.join("park-reasons.yaml"), &no_actor).unwrap();
        let e = load_park_reasons(&wf).unwrap_err().to_string();
        assert!(e.contains("actors"), "{e}");

        // A blank platform word is refused by name as well.
        let blank_actor = factory.replace("platform: \"платформа\"", "platform: \"\"");
        assert_ne!(factory, blank_actor);
        fs::write(wf.join("park-reasons.yaml"), &blank_actor).unwrap();
        let e = load_park_reasons(&wf).unwrap_err().to_string();
        assert!(e.contains("actors.platform"), "{e}");
    }
}
