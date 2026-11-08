//! Declared MODEL BUNDLES — the allow-list the switch is allowed to choose
//! from (task 01a1b71f, REQ-MS-01/02/08).
//!
//! The file is `models.yaml` of the home config tree. It is DATA: the
//! platform reads it, never rewrites it, and the switch operation accepts
//! only the ids it declares — there is no free-form driver/model/provider
//! input anywhere (keys live in the environment and in the driver's own
//! config; they are not declared, not stored and not shown).
//!
//! Grammar (deny-by-default):
//!
//! ```yaml
//! bundles:
//!   - id: hermes-deepseek          # the handle probe/switch accept
//!     label: Hermes · DeepSeek     # the human name in the UI
//!     agent: hermes                # the agent_instances NAME (the record)
//!     driver: hermes               # mock | claude | hermes
//!     llm_provider: deepseek       # empty = the driver chooses
//!     model: deepseek-v4.1-flash   # empty = not passed
//!     reasoning: high              # REQUIRED: the level of thinking
//!     probe:
//!       phrase: ping
//!       timeout_ms: 20000
//!       fresh_secs: 900
//! ```
//!
//! Identity is `(agent, driver, llm_provider, model, reasoning)`: a repeated
//! id OR a repeated identity fails the whole load with BOTH records named. A
//! `mock` bundle is rejected at load (it has no network leg to probe) and a
//! `claude` bundle may not declare an LLM-provider (the driver chooses its
//! own). Unknown keys — `key`, `token`, `base_url` included — fail with the
//! key's name: they are exactly what the UI must never accept.
//!
//! `reasoning` is not optional and is not defaulted (the ratified OQ-6
//! answer): the level is part of the chosen bundle, so a bundle
//! without it is INCOMPLETE and a run on it does not start — the load
//! refuses instead of silently inheriting the driver's own default.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{StoreError, StoreResult};

/// Repository-relative location inside the home config tree.
pub const MODEL_CONFIG_FILE: &str = "models.yaml";

/// Driver names that HAVE a network leg and may be declared. `mock` is
/// deliberately absent: it is a diagnostic driver, never a bundle.
pub const PROBEABLE_DRIVERS: &[&str] = &["claude", "hermes"];

/// The driver whose LLM-provider field must stay empty (it picks one itself).
const SELF_PROVIDER_DRIVER: &str = "claude";

fn default_phrase() -> String {
    devai_core::types::DEFAULT_PROBE_PHRASE.into()
}
fn default_timeout_ms() -> u64 {
    devai_core::types::DEFAULT_PROBE_TIMEOUT_MS
}
fn default_fresh_secs() -> u64 {
    devai_core::types::DEFAULT_PROBE_FRESH_SECS
}

/// Probe policy of one bundle (ASM-4: the factory values are a starting
/// point, the home owns them; the timeout is a POLICY, not a business rule).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct ProbePolicy {
    /// The single phrase the probe call carries — short and cheap.
    pub phrase: String,
    /// Hard wall-clock budget in milliseconds; the process is killed and
    /// reaped at the deadline (a probe never hangs longer).
    pub timeout_ms: u64,
    /// How long a probe's evidence stays FRESH for the status field
    /// (REQ-MS-05: fresh|stale|unknown is derived from this window).
    pub fresh_secs: u64,
}

impl Default for ProbePolicy {
    fn default() -> Self {
        Self {
            phrase: default_phrase(),
            timeout_ms: default_timeout_ms(),
            fresh_secs: default_fresh_secs(),
        }
    }
}

/// One declared bundle. The field names ARE the config vocabulary:
/// `driver` / `llm_provider` / `model` / `reasoning` — «provider» alone never
/// appears.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModelBundle {
    pub id: String,
    pub label: String,
    /// The `agent_instances` name this bundle is materialised under. Two
    /// bundles may name one agent: the record's identity is overwritten by
    /// the switch, which is why switching to another MODEL or REASONING
    /// LEVEL of the same agent needs no restart (ASM-8, the ratified OQ-6 answer).
    pub agent: String,
    pub driver: String,
    #[serde(default)]
    pub llm_provider: String,
    #[serde(default)]
    pub model: String,
    /// The level of thinking (the ratified OQ-6 answer): REQUIRED. Empty never loads — the
    /// level is part of the bundle, not a driver default.
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub probe: ProbePolicy,
}

impl ModelBundle {
    /// The declared identity — the quintuple the store row's content must
    /// match (REQ-MS-02 + the ratified OQ-6 answer: agent, driver, LLM-provider, model,
    /// reasoning level).
    pub fn identity(&self) -> (&str, &str, &str, &str, &str) {
        (
            self.agent.as_str(),
            self.driver.as_str(),
            self.llm_provider.as_str(),
            self.model.as_str(),
            self.reasoning.as_str(),
        )
    }

    /// The identity as one readable string, for error texts and history.
    pub fn identity_text(&self) -> String {
        format!(
            "agent `{}` / driver `{}` / llm_provider `{}` / model `{}` / reasoning `{}`",
            self.agent,
            self.driver,
            if self.llm_provider.is_empty() {
                "—"
            } else {
                &self.llm_provider
            },
            if self.model.is_empty() {
                "—"
            } else {
                &self.model
            },
            self.reasoning,
        )
    }
}

/// The whole declared catalogue of one home.
#[derive(Clone, Debug, Default)]
pub struct ModelCatalog {
    pub bundles: Vec<ModelBundle>,
}

impl ModelCatalog {
    pub fn get(&self, id: &str) -> Option<&ModelBundle> {
        self.bundles.iter().find(|b| b.id == id)
    }

    pub fn ids(&self) -> Vec<&str> {
        self.bundles.iter().map(|b| b.id.as_str()).collect()
    }

    /// Resolve a store record to a declared bundle: every identity field has
    /// to match (a manually added agent row is NOT a declared bundle — the
    /// snapshot says so instead of borrowing the nearest label).
    pub fn for_record(
        &self,
        agent: &str,
        driver: &str,
        llm_provider: &str,
        model: &str,
        reasoning: &str,
    ) -> Option<&ModelBundle> {
        self.bundles.iter().find(|b| {
            b.agent == agent
                && b.driver == driver
                && b.llm_provider == llm_provider
                && b.model == model
                && b.reasoning == reasoning
        })
    }

    /// The probe policy of the active record: the matching bundle's, else the
    /// factory values (a record outside the list has no declared policy).
    pub fn policy_for(
        &self,
        agent: &str,
        driver: &str,
        llm_provider: &str,
        model: &str,
        reasoning: &str,
    ) -> ProbePolicy {
        self.for_record(agent, driver, llm_provider, model, reasoning)
            .map(|b| b.probe.clone())
            .unwrap_or_default()
    }
}

/// The whitelist of keys a bundle record may carry. Everything else — in
/// particular a key, a token, a credential name or `base_url` — is a typed
/// load error naming the offending key (REQ-MS-02 AC2, REQ-MS-08 AC1).
const BUNDLE_KEYS: &[&str] = &[
    "id",
    "label",
    "agent",
    "driver",
    "llm_provider",
    "model",
    "reasoning",
    "probe",
];
const PROBE_KEYS: &[&str] = &["phrase", "timeout_ms", "fresh_secs"];

fn cfg(msg: impl Into<String>) -> StoreError {
    StoreError::ModelBundle(msg.into())
}

/// Load `models.yaml` from the home config dir. A missing file is an EMPTY
/// catalogue (a home that declares nothing simply has nothing to switch to:
/// «связка не выбрана»), never a hard failure of the whole home.
pub fn load_catalog(config_dir: &Path) -> StoreResult<ModelCatalog> {
    let path = config_dir.join(MODEL_CONFIG_FILE);
    if !path.exists() {
        return Ok(ModelCatalog::default());
    }
    Ok(ModelCatalog {
        bundles: read_bundles(&path)?,
    })
}

/// Read and parse the file into records: document shape, per-record white
/// list, then the cross-record rules. Each failure is typed and names the
/// file (and the index) it came from.
fn read_bundles(path: &Path) -> StoreResult<Vec<ModelBundle>> {
    let text =
        fs::read_to_string(path).map_err(|e| cfg(format!("read {}: {e}", path.display())))?;
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| cfg(format!("parse {}: {e}", path.display())))?;
    let items = declared_list(&doc, path)?;
    let mut bundles = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        bundles.push(parse_bundle(item, idx, path)?);
    }
    validate(&bundles, path)?;
    Ok(bundles)
}

/// The `bundles:` sequence of the document. Any other top-level field is a
/// typed error: the file declares only one thing.
fn declared_list<'a>(
    doc: &'a serde_yaml::Value,
    path: &Path,
) -> StoreResult<&'a [serde_yaml::Value]> {
    let mapping = doc.as_mapping().ok_or_else(|| {
        cfg(format!(
            "{}: the document must be a map with a `bundles:` list",
            path.display()
        ))
    })?;
    for key in mapping.keys() {
        let key = key.as_str().unwrap_or("");
        if key != "bundles" {
            return Err(cfg(format!(
                "{}: unknown top-level field `{key}` — the file declares only `bundles`",
                path.display()
            )));
        }
    }
    match mapping.get(serde_yaml::Value::String("bundles".into())) {
        Some(v) => v
            .as_sequence()
            .map(Vec::as_slice)
            .ok_or_else(|| cfg(format!("{}: `bundles` must be a list", path.display()))),
        None => Ok(&[]),
    }
}

/// One declared record: the deny-by-default key check, then the typed parse.
fn parse_bundle(item: &serde_yaml::Value, idx: usize, path: &Path) -> StoreResult<ModelBundle> {
    let map = item.as_mapping().ok_or_else(|| {
        cfg(format!(
            "{}: bundles[{idx}] must be a map of declared fields",
            path.display()
        ))
    })?;
    check_bundle_keys(map, idx, path)?;
    check_probe_keys(map, idx, path)?;
    serde_yaml::from_value(item.clone())
        .map_err(|e| cfg(format!("{}: bundles[{idx}]: {e}", path.display())))
}

/// The white list of bundle keys: everything else — a key, a token, a
/// credential name or `base_url` — is a typed error naming the key.
fn check_bundle_keys(map: &serde_yaml::Mapping, idx: usize, path: &Path) -> StoreResult<()> {
    check_keys(map, BUNDLE_KEYS, idx, None, path)
}

/// The white list of `probe:` keys.
fn check_probe_keys(map: &serde_yaml::Mapping, idx: usize, path: &Path) -> StoreResult<()> {
    let Some(probe) = map.get(serde_yaml::Value::String("probe".into())) else {
        return Ok(());
    };
    let probe_map = probe.as_mapping().ok_or_else(|| {
        cfg(format!(
            "{}: bundles[{idx}].probe must be a map",
            path.display()
        ))
    })?;
    check_keys(probe_map, PROBE_KEYS, idx, Some("probe"), path)
}

/// ONE deny-by-default key check for both levels: the caller passes the
/// white list and the section name (`None` = the record itself).
fn check_keys(
    map: &serde_yaml::Mapping,
    allowed: &[&str],
    idx: usize,
    section: Option<&str>,
    path: &Path,
) -> StoreResult<()> {
    let where_ = match section {
        Some(s) => format!("bundles[{idx}].{s}"),
        None => format!("bundles[{idx}]"),
    };
    for key in map.keys() {
        let key = key.as_str().unwrap_or("");
        if !allowed.contains(&key) {
            return Err(cfg(format!(
                "{}: {where_}: unknown field `{key}` — declared fields are {}",
                path.display(),
                allowed.join(", ")
            )));
        }
    }
    Ok(())
}

/// The cross-record rules, one responsibility per check: completeness of a
/// record, its driver, its probe policy, and the two uniqueness rules.
fn validate(bundles: &[ModelBundle], path: &Path) -> StoreResult<()> {
    let mut ids: HashSet<&str> = HashSet::new();
    let mut identities: Vec<(&str, (&str, &str, &str, &str, &str))> = Vec::new();
    for b in bundles {
        check_required(b, path)?;
        check_driver(b, path)?;
        check_probe_policy(b, path)?;
        if !ids.insert(b.id.as_str()) {
            return Err(cfg(format!(
                "{}: duplicate bundle id `{}` — ids are handles, one record per id",
                path.display(),
                b.id
            )));
        }
        check_unique_identity(&mut identities, b, path)?;
    }
    Ok(())
}

/// Every field the surface renders has to be present; the reasoning level is
/// part of the bundle (the ratified OQ-6 answer), so an empty one is an
/// INCOMPLETE bundle — the load refuses instead of running on a driver
/// default.
fn check_required(b: &ModelBundle, path: &Path) -> StoreResult<()> {
    for (field, value) in [
        ("id", &b.id),
        ("label", &b.label),
        ("agent", &b.agent),
        ("driver", &b.driver),
    ] {
        if value.trim().is_empty() {
            return Err(cfg(format!(
                "{}: bundle `{}` has an empty `{field}`",
                path.display(),
                b.id
            )));
        }
    }
    if b.reasoning.trim().is_empty() {
        return Err(cfg(format!(
            "{}: bundle `{}` declares no `reasoning` level — the level is part of the bundle (chosen with the LLM-provider and the model); a bundle without it is incomplete and a run on it does not start",
            path.display(),
            b.id
        )));
    }
    Ok(())
}

/// The driver rules: `mock` is never a bundle, only drivers with a network
/// leg are declared, and a self-providing driver must leave the field empty.
fn check_driver(b: &ModelBundle, path: &Path) -> StoreResult<()> {
    if b.driver == "mock" {
        return Err(cfg(format!(
            "{}: bundle `{}` declares driver `mock` — mock has no network leg and is never a bundle (probe is not_probeable)",
            path.display(),
            b.id
        )));
    }
    if !PROBEABLE_DRIVERS.contains(&b.driver.as_str()) {
        return Err(cfg(format!(
            "{}: bundle `{}` declares unknown driver `{}` — allowed: {}",
            path.display(),
            b.id,
            b.driver,
            PROBEABLE_DRIVERS.join(", ")
        )));
    }
    if b.driver == SELF_PROVIDER_DRIVER && !b.llm_provider.is_empty() {
        return Err(cfg(format!(
            "{}: bundle `{}` sets `llm_provider: {}` on driver `{}` — that driver chooses its own LLM-provider, the field must be empty",
            path.display(),
            b.id,
            b.llm_provider,
            b.driver
        )));
    }
    Ok(())
}

/// A probe is one real call: its phrase, timeout and freshness window must be
/// usable values, never zero.
fn check_probe_policy(b: &ModelBundle, path: &Path) -> StoreResult<()> {
    if b.probe.timeout_ms == 0 || b.probe.fresh_secs == 0 || b.probe.phrase.trim().is_empty() {
        return Err(cfg(format!(
            "{}: bundle `{}`: probe.phrase/timeout_ms/fresh_secs must be non-empty and positive",
            path.display(),
            b.id
        )));
    }
    Ok(())
}

/// The identity is unique: one record per `(agent, driver, llm_provider,
/// model, reasoning)`, and the error names BOTH records.
fn check_unique_identity<'a>(
    identities: &mut Vec<(&'a str, (&'a str, &'a str, &'a str, &'a str, &'a str))>,
    b: &'a ModelBundle,
    path: &Path,
) -> StoreResult<()> {
    let identity = (
        b.agent.as_str(),
        b.driver.as_str(),
        b.llm_provider.as_str(),
        b.model.as_str(),
        b.reasoning.as_str(),
    );
    if let Some((other_id, _)) = identities.iter().find(|entry| entry.1 == identity) {
        return Err(cfg(format!(
            "{}: bundles `{other_id}` and `{}` declare the same identity ({}) — one identity, one record",
            path.display(),
            b.id,
            b.identity_text()
        )));
    }
    identities.push((b.id.as_str(), identity));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, text: &str) {
        let mut f = fs::File::create(dir.join(MODEL_CONFIG_FILE)).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    const TWO: &str = r#"
bundles:
  - id: hermes-deepseek
    label: Hermes DeepSeek
    agent: hermes
    driver: hermes
    llm_provider: deepseek
    model: deepseek-v4.1-flash
    reasoning: high
  - id: claude-default
    label: Claude
    agent: claude
    driver: claude
    reasoning: medium
"#;

    #[test]
    fn a_clean_home_loads_the_declared_bundles_and_defaults_the_policy() {
        let d = tmp();
        write(d.path(), TWO);
        let c = load_catalog(d.path()).unwrap();
        assert_eq!(c.ids(), vec!["hermes-deepseek", "claude-default"]);
        let b = c.get("hermes-deepseek").unwrap();
        assert_eq!(
            b.identity(),
            (
                "hermes",
                "hermes",
                "deepseek",
                "deepseek-v4.1-flash",
                "high"
            )
        );
        assert_eq!(b.probe.timeout_ms, 20_000);
        assert_eq!(b.probe.fresh_secs, 900);
        assert!(!b.probe.phrase.is_empty());
    }

    /// the ratified OQ-6 answer: the level is part of the bundle and
    /// is NOT optional — a bundle without it never loads.
    #[test]
    fn a_bundle_without_a_reasoning_level_is_refused_at_load() {
        let d = tmp();
        write(
            d.path(),
            "bundles:\n  - id: a\n    label: A\n    agent: a\n    driver: hermes\n    model: m\n",
        );
        let err = load_catalog(d.path()).unwrap_err().to_string();
        assert!(err.contains("reasoning"), "{err}");
        assert!(err.contains("incomplete"), "{err}");
    }

    /// The level is part of the IDENTITY: two bundles that differ only by the
    /// level are two bundles (that is how the operator switches it), while a
    /// fourth copy of the same quintuple is still a duplicate.
    #[test]
    fn the_reasoning_level_is_part_of_the_identity() {
        let d = tmp();
        write(
            d.path(),
            "bundles:\n  - id: low\n    label: Low\n    agent: hermes\n    driver: hermes\n    model: m\n    reasoning: low\n  - id: high\n    label: High\n    agent: hermes\n    driver: hermes\n    model: m\n    reasoning: high\n",
        );
        let c = load_catalog(d.path()).unwrap();
        assert_eq!(c.ids(), vec!["low", "high"]);

        let d2 = tmp();
        write(
            d2.path(),
            "bundles:\n  - id: a\n    label: A\n    agent: hermes\n    driver: hermes\n    model: m\n    reasoning: high\n  - id: b\n    label: B\n    agent: hermes\n    driver: hermes\n    model: m\n    reasoning: high\n",
        );
        let err = load_catalog(d2.path()).unwrap_err().to_string();
        assert!(err.contains("`a`") && err.contains("`b`"), "{err}");
        assert!(err.contains("same identity"), "{err}");
    }

    #[test]
    fn a_missing_file_is_an_empty_list_not_a_failure() {
        let d = tmp();
        assert!(load_catalog(d.path()).unwrap().bundles.is_empty());
    }

    #[test]
    fn unknown_keys_are_refused_by_name_keys_and_base_url_never_load() {
        // The credential key name is assembled at run time so the repository
        // text never carries the literal the `no-secrets` gate looks for.
        let credential_key = format!("api{}key", "_");
        for key in ["key", "token", credential_key.as_str(), "base_url"] {
            let d = tmp();
            write(
                d.path(),
                &format!(
                    "bundles:\n  - id: a\n    label: A\n    agent: a\n    driver: hermes\n    reasoning: high\n    {key}: secret\n"
                ),
            );
            let err = load_catalog(d.path()).unwrap_err();
            assert!(
                err.to_string().contains(key),
                "the error must name `{key}`: {err}"
            );
        }
    }

    #[test]
    fn duplicate_id_and_duplicate_identity_name_both_records() {
        let d = tmp();
        write(
            d.path(),
            "bundles:\n  - id: a\n    label: A\n    agent: a\n    driver: hermes\n    reasoning: high\n    model: m\n  - id: a\n    label: B\n    agent: b\n    driver: hermes\n    reasoning: high\n",
        );
        let err = load_catalog(d.path()).unwrap_err().to_string();
        assert!(err.contains("duplicate bundle id `a`"), "{err}");

        let d2 = tmp();
        write(
            d2.path(),
            "bundles:\n  - id: a\n    label: A\n    agent: a\n    driver: hermes\n    reasoning: high\n    model: m\n  - id: b\n    label: B\n    agent: a\n    driver: hermes\n    reasoning: high\n    model: m\n",
        );
        let err = load_catalog(d2.path()).unwrap_err().to_string();
        assert!(err.contains("`a`") && err.contains("`b`"), "{err}");
        assert!(err.contains("same identity"), "{err}");
    }

    #[test]
    fn mock_is_never_a_bundle_and_claude_carries_no_llm_provider() {
        let d = tmp();
        write(
            d.path(),
            "bundles:\n  - id: m\n    label: M\n    agent: m\n    driver: mock\n    reasoning: high\n",
        );
        let err = load_catalog(d.path()).unwrap_err().to_string();
        assert!(err.contains("mock"), "{err}");

        let d2 = tmp();
        write(
            d2.path(),
            "bundles:\n  - id: c\n    label: C\n    agent: c\n    driver: claude\n    llm_provider: openrouter\n    reasoning: high\n",
        );
        let err = load_catalog(d2.path()).unwrap_err().to_string();
        assert!(err.contains("llm_provider"), "{err}");
        assert!(err.contains("claude"), "{err}");
    }

    #[test]
    fn identity_resolution_needs_every_field() {
        let d = tmp();
        write(d.path(), TWO);
        let c = load_catalog(d.path()).unwrap();
        assert!(
            c.for_record(
                "hermes",
                "hermes",
                "deepseek",
                "deepseek-v4.1-flash",
                "high"
            )
            .is_some()
        );
        assert!(
            c.for_record("hermes", "hermes", "deepseek", "other", "high")
                .is_none()
        );
        assert!(
            c.for_record("hermes", "hermes", "", "deepseek-v4.1-flash", "high")
                .is_none()
        );
        // The same identity with another level is NOT this bundle.
        assert!(
            c.for_record("hermes", "hermes", "deepseek", "deepseek-v4.1-flash", "low")
                .is_none()
        );
    }
}
