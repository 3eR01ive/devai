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

use devai_core::types::{ContractRef, RoleDefinition, Timestamp, new_id};
use serde::{Deserialize, Serialize};

use crate::{Store, StoreError, StoreResult};

// Embedded factory defaults — written into the home config on first init.
const DEFAULT_FILES: &[(&str, &str)] = &[
    ("prompts/common.yaml", include_str!("../../../config/prompts/common.yaml")),
    ("criteria/review.yaml", include_str!("../../../config/criteria/review.yaml")),
    ("evaluation/quality.yaml", include_str!("../../../config/evaluation/quality.yaml")),
    ("roles/project-intelligence.yaml", include_str!("../../../config/roles/project-intelligence.yaml")),
    ("roles/requirements-analyst.yaml", include_str!("../../../config/roles/requirements-analyst.yaml")),
    ("roles/requirements-reviewer.yaml", include_str!("../../../config/roles/requirements-reviewer.yaml")),
    ("roles/implementation-agent.yaml", include_str!("../../../config/roles/implementation-agent.yaml")),
    ("roles/quality-auditor.yaml", include_str!("../../../config/roles/quality-auditor.yaml")),
    ("roles/quality-aggregator.yaml", include_str!("../../../config/roles/quality-aggregator.yaml")),
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
        self.evaluations.iter().flat_map(|e| e.checks.iter()).collect()
    }

    /// Provenance identity `name@version` recorded in every workspace
    /// revision. Falls back to `project` when no name is declared anywhere.
    pub fn identity(&self) -> String {
        format!("{}@{}", self.name.as_deref().unwrap_or(&self.project), self.version)
    }

    /// Highest severity among the check's detectable error types (V8 source).
    /// SEVERITIES is ordered most-severe first, so a lower index wins.
    pub fn worst_severity(&self, c: &CheckDef) -> Option<&str> {
        let rank = |s: &str| SEVERITIES.iter().position(|x| *x == s);
        let mut best: Option<&str> = None;
        for t in &c.detects {
            let Some(sev) = self.error_types.get(t).map(|e| e.severity.as_str()) else { continue };
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
        self.worst_severity(c).and_then(|sev| self.scoring.floors.get(sev).and_then(|v| *v))
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
                return Err(format!("error_type `{t}`: severity `{}` not in {SEVERITIES:?}", e.severity));
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
                return Err(format!("check `{}`: reviewer `{}` invalid", c.id, c.reviewer));
            }
            // V1: every detected error type must be declared.
            for t in &c.detects {
                if !self.error_types.contains_key(t) {
                    return Err(format!("check `{}` detects undeclared error type `{t}` (V1)", c.id));
                }
            }
            // V3: mechanical/hybrid need at least one probe.
            if matches!(c.verify.as_str(), "mechanical" | "hybrid") && c.probes.is_empty() {
                return Err(format!("check `{}` is {} without probe (V3)", c.id, c.verify));
            }
            // V4: rubric/hybrid need >=3 anchors incl. 1.0 and 0.0.
            if matches!(c.verify.as_str(), "rubric" | "hybrid") {
                if c.anchors.len() < 3 || !c.anchors.contains_key("1.0") || !c.anchors.contains_key("0.0") {
                    return Err(format!("check `{}`: anchors must have >=3 gradations incl. \"1.0\" and \"0.0\" (V4)", c.id));
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
        self.profiles.iter().find(|p| p.applies_to_roles.iter().any(|r| r == slug))
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
            let _ = writeln!(ins, "\n[{}] ({}; detects {}; weight {})", c.id, c.verify, c.detects.join(", "), profile.weight_of(&c.id));
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
                let rendered: Vec<String> = anchors.into_iter().map(|(k, v)| format!("{k} — {}", v.trim())).collect();
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
                    .map(|(s, v)| format!("{s}: {}", v.map(|x| x.to_string()).unwrap_or_else(|| "none".into())))
                    .collect();
                f.join(", ")
            }
        );
    }

    /// Compile a role file into a stored RoleDefinition: base instructions
    /// plus the common rules and, when defined, the basic-checks checklist
    /// or the evaluation profile (a profile supersedes flat criteria).
    pub fn build_role(&self, rf: &RoleFile, now: Timestamp) -> RoleDefinition {
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
        RoleDefinition {
            id: new_id(),
            slug: rf.slug.clone(),
            version: rf.version.max(1),
            name: rf.name.clone(),
            purpose: rf.purpose.clone(),
            instructions: ins,
            tools: rf.tools.clone(),
            input_contract: parse_contract_ref(rf.input_contract.as_deref()).ok().flatten(),
            output_contract: parse_contract_ref(rf.output_contract.as_deref()).ok().flatten(),
            done_criteria: rf.done_criteria.clone(),
            created_at: now,
        }
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
        fs::create_dir_all(path.parent().ok_or_else(|| cfg_err(format!("config path `{rel}` has no parent")))?)?;
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
        profile.validate().map_err(|e| cfg_err(format!("{}: {e}", p.display())))?;
        profiles.push(profile);
    }
    Ok(profiles)
}

fn parse_yaml<T: DeserializeOwnedPlus>(path: &Path) -> StoreResult<T> {
    let text = fs::read_to_string(path).map_err(|e| cfg_err(format!("read {}: {e}", path.display())))?;
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
    Ok(Config { roles, criteria, common, profiles })
}

/// Upsert config roles into the store, versioning on content change.
/// Returns how many role versions were created.
pub fn seed_roles(store: &Store, cfg: &Config, now: Timestamp) -> StoreResult<usize> {
    let mut changed = 0;
    for rf in &cfg.roles {
        let rendered = cfg.build_role(rf, now);
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
        let rf = cfg.roles.iter().find(|r| r.slug == "requirements-reviewer").unwrap();
        let role = cfg.build_role(rf, Utc::now());
        assert!(role.instructions.contains("[RV1|blocker]"), "{}", role.instructions);
        assert!(role.instructions.contains("Verdict rule:"));
        assert!(role.instructions.contains("Workflow rules:"));
        // Analyst has no criteria — only the common rules block.
        let arf = cfg.roles.iter().find(|r| r.slug == "requirements-analyst").unwrap();
        let arole = cfg.build_role(arf, Utc::now());
        assert!(!arole.instructions.contains("Verdict rule:"));
        assert!(arole.instructions.contains("Workflow rules:"));
        assert_eq!(arole.output_contract.as_ref().unwrap().name, "requirements-specification");
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
        let lb = p.checks().into_iter().find(|c| c.id == "arch/layer-boundaries").unwrap();
        assert_eq!(p.worst_severity(lb), Some("catastrophic"));
        assert_eq!(p.floor_for(lb), Some(0.7));
        // Auditor instructions carry the profile-compiled rubric checklist.
        let rf = cfg.roles.iter().find(|r| r.slug == "quality-auditor").unwrap();
        let role = cfg.build_role(rf, Utc::now());
        assert!(role.instructions.contains("Evaluation profile `devai`"));
        assert!(role.instructions.contains("[arch/idiom-consistency]"), "{}", role.instructions);
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
        let restored = p.scoring.weights.remove("arch/no-magic").expect("weight present");
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
        assert_eq!(seed_roles(&store, &cfg, now).unwrap(), 0, "idempotent re-seed");

        // The factory YAML declares its own role version (71d416b moved
        // quality-auditor to @2): the assertions track THAT declared number,
        // never a hardcoded literal that rots on the next declarative bump.
        let declared = cfg.roles.iter().find(|r| r.slug == "quality-auditor").unwrap().version;

        // Edit the evaluation profile (raise theta) → next seed bumps just the auditor.
        let mut cfg2 = cfg.clone();
        cfg2.profiles.first_mut().unwrap().scoring.theta = 0.85;
        assert_eq!(seed_roles(&store, &cfg2, now).unwrap(), 1);

        let q = store.get_role("quality-auditor").unwrap().unwrap();
        assert_eq!(q.version, declared + 1, "latest version wins for new executions");
        assert!(q.instructions.contains("theta 0.85"), "{}", q.instructions);
        // The factory-declared row remains in the table — history is never overwritten.
        let all = store.list_roles().unwrap();
        assert!(all.iter().any(|r| r.slug == "quality-auditor" && r.version == declared));
        assert!(all.iter().any(|r| r.slug == "quality-auditor" && r.version == declared + 1));
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
            written.iter().any(|p| p.file_name().is_some_and(|n| n == "quality.yaml")
                && p.parent().is_some_and(|d| d.file_name().is_some_and(|n| n == "evaluation"))),
            "re-init must bring the new factory profile into an old home: {written:?}"
        );
        assert_eq!(fs::read_to_string(cfgdir.join("criteria/review.yaml")).unwrap(), "mine");
    }
}
