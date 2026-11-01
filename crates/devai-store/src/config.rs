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
//!   prompts/common.yaml       # base role rules
//!   criteria/review.yaml      # basic spec-acceptance checks (reviewer)
//!   criteria/quality.yaml     # basic QA checks (auditor)
//!   roles/<slug>.yaml         # role definitions
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use devai_core::types::{ContractRef, RoleDefinition, Timestamp, new_id};
use serde::{Deserialize, Serialize};

use crate::{Store, StoreError, StoreResult};

// Embedded factory defaults — written into the home config on first init.
const DEFAULT_FILES: &[(&str, &str)] = &[
    ("prompts/common.yaml", include_str!("../../../config/prompts/common.yaml")),
    ("criteria/review.yaml", include_str!("../../../config/criteria/review.yaml")),
    ("criteria/quality.yaml", include_str!("../../../config/criteria/quality.yaml")),
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
}

impl Config {
    fn criteria_for(&self, slug: &str) -> Option<&Criteria> {
        self.criteria.iter().find(|c| c.applies_to == slug)
    }

    /// Compile a role file into a stored RoleDefinition: base instructions
    /// plus the common rules and, when defined, the basic-checks checklist.
    pub fn build_role(&self, rf: &RoleFile, now: Timestamp) -> RoleDefinition {
        let mut ins = rf.instructions.trim().to_string();
        if !self.common.rules.is_empty() {
            ins.push_str("\n\nWorkflow rules:\n");
            for r in &self.common.rules {
                ins.push_str(&format!("- {r}\n"));
            }
            ins = ins.trim_end().to_string();
        }
        if let Some(c) = self.criteria_for(&rf.slug) {
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

/// Write the factory config into `dir` if the roles directory is missing.
pub fn materialize_if_absent(dir: &Path) -> StoreResult<Vec<PathBuf>> {
    if dir.join("roles").is_dir() {
        return Ok(Vec::new());
    }
    let mut written = Vec::new();
    for (rel, text) in DEFAULT_FILES {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap())?;
        // Never clobber a user-edited file on partial existing config.
        if path.exists() {
            continue;
        }
        fs::write(&path, text)?;
        written.push(path);
    }
    Ok(written)
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
    Ok(Config { roles, criteria, common })
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
    fn reseed_bumps_version_only_on_change_and_never_deletes() {
        let dir = tempfile::TempDir::new().unwrap();
        materialize_if_absent(&dir.path().join("config")).unwrap();
        let cfg = load(&dir.path().join("config")).unwrap();
        let store = Store::open(dir.path()).unwrap();
        let now = Utc::now();
        assert_eq!(seed_roles(&store, &cfg, now).unwrap(), 6);
        assert_eq!(seed_roles(&store, &cfg, now).unwrap(), 0, "idempotent re-seed");

        // Edit one criteria file → next seed bumps just that role.
        let mut cfg2 = cfg.clone();
        let c = cfg2.criteria.iter_mut().find(|c| c.applies_to == "quality-auditor").unwrap();
        c.checks.push(Check { id: "QA9".into(), text: "extra rule".into(), severity: "minor".into() });
        c.verdict_rule = "always escalate".into();
        assert_eq!(seed_roles(&store, &cfg2, now).unwrap(), 1);

        let q = store.get_role("quality-auditor").unwrap().unwrap();
        assert_eq!(q.version, 2, "latest version wins for new executions");
        assert!(q.instructions.contains("QA9"));
        // v1 remains in the table — history is never overwritten.
        let all = store.list_roles().unwrap();
        assert!(all.iter().any(|r| r.slug == "quality-auditor" && r.version == 1));
        assert!(all.iter().any(|r| r.slug == "quality-auditor" && r.version == 2));
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
}
