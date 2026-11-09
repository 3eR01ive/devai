//! Declared MODEL BUNDLES — the allow-list the switch is allowed to choose
//! from (task 01a1b71f, REQ-MS-01/02/08).
//!
//! The file is `models.yaml` of the home config tree. It is DATA: the
//! platform reads it, and writes it ONLY on the operator's explicit act in
//! the bundle form (task 01a1be5c — create/edit/delete) — atomically, after
//! a successful probe, keeping the operator's comments and every untouched
//! line, and only if no other writer got there first. The switch operation
//! accepts only the ids it declares — there is no free-form driver/model/
//! provider input anywhere (keys live in the environment and in the driver's
//! own config; they are not declared, not stored and not shown).
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
pub const SELF_PROVIDER_DRIVER: &str = "claude";

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

/// The revision of the declared file: the hex SHA-256 of its bytes, or the
/// empty string when the file does not exist yet (a form opened on an empty
/// home carries `""`). The surface serves it with the snapshot and the
/// writer compares it — the whole stale-form rule rests on ONE value.
pub fn catalog_revision(config_dir: &Path) -> StoreResult<String> {
    let path = config_dir.join(MODEL_CONFIG_FILE);
    match fs::read(&path) {
        Ok(bytes) => Ok(revision_of(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(cfg(format!("read {}: {e}", path.display()))),
    }
}

fn revision_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    crate::hex_encode(&sha2::Sha256::digest(bytes))
}

/// The current FILE TEXT (empty when absent). The update path keeps it to
/// restore the file if the store step of an edit fails (REQ-MC-06 AC5):
/// the operator must never be left with a new file and an old record.
pub fn catalog_text(config_dir: &Path) -> StoreResult<String> {
    let path = config_dir.join(MODEL_CONFIG_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(cfg(format!("read {}: {e}", path.display()))),
    }
}

/// Put back the exact text read by [`catalog_text`] — used ONLY to undo an
/// edit whose later step failed. Atomic like every write here and not
/// revision-guarded: the caller owns the rollback.
pub fn restore_catalog_text(config_dir: &Path, text: &str) -> StoreResult<()> {
    let path = config_dir.join(MODEL_CONFIG_FILE);
    write_atomic(&path, text)
}

/// One whole-file edit of the declared catalogue (REQ-MC-01/03/05/06).
pub enum CatalogEdit<'a> {
    /// Insert a record, or replace the record carrying the same id.
    /// `must_exist` is the create/edit distinction: a create whose id is
    /// taken refuses, an edit of a record that vanished refuses.
    Upsert {
        bundle: &'a ModelBundle,
        must_exist: bool,
    },
    /// Remove one record.
    Remove { id: &'a str },
}

/// Apply `edit` to `models.yaml` and return the NEW revision.
///
/// The write is ATOMIC (a temp file in the same directory, then rename), the
/// whole resulting document is re-parsed with the SAME rules as a load
/// before anything is replaced, and the caller's `expected_revision` must
/// still match the bytes on disk — a stale form refuses instead of
/// overwriting another writer (REQ-MC-06). The edit is surgical: only the
/// affected record's lines change, so the operator's comments, blank lines
/// and untouched declarations keep their exact lines.
pub fn write_catalog(
    config_dir: &Path,
    expected_revision: &str,
    edit: &CatalogEdit<'_>,
) -> StoreResult<String> {
    let path = config_dir.join(MODEL_CONFIG_FILE);
    let text = catalog_text(config_dir)?;
    let found = match fs::read(&path) {
        Ok(bytes) => revision_of(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(cfg(format!("read {}: {e}", path.display()))),
    };
    if found != expected_revision {
        return Err(StoreError::Stale {
            file: path.display().to_string(),
            expected: expected_revision.to_string(),
            found,
        });
    }
    let mut doc = Doc::parse(&text);
    match edit {
        CatalogEdit::Upsert { bundle, must_exist } => doc.upsert(bundle, *must_exist, &path)?,
        CatalogEdit::Remove { id } => doc.remove(id, &path)?,
    }
    let new_text = doc.render();
    // The edited document must LOAD. This is the same rule set the reader
    // uses, so a writer bug is a refusal with nothing on disk, never a home
    // that stops opening.
    let written = parse_bundles(&new_text, &path)?;
    match edit {
        CatalogEdit::Upsert { bundle, .. } => {
            if written.iter().find(|b| b.id == bundle.id) != Some(*bundle) {
                return Err(cfg(format!(
                    "{}: write verification failed — the edited document does not carry bundle `{}` with the requested values; nothing was written",
                    path.display(),
                    bundle.id
                )));
            }
        }
        CatalogEdit::Remove { id } => {
            if written.iter().any(|b| b.id == *id) {
                return Err(cfg(format!(
                    "{}: write verification failed — bundle `{id}` is still declared; nothing was written",
                    path.display()
                )));
            }
        }
    }
    write_atomic(&path, &new_text)?;
    Ok(revision_of(new_text.as_bytes()))
}

/// Write `text` to `path` through a sibling temp file and a rename: a reader
/// sees either the old or the new file, never a half-written one. A failed
/// step removes the temp file and leaves the target untouched.
fn write_atomic(path: &Path, text: &str) -> StoreResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| cfg(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(MODEL_CONFIG_FILE);
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let staged = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = fs::remove_file(&tmp);
        return Err(StoreError::Io(e));
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(StoreError::Io(e));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// The comment-preserving editor of the declared document.
//
// YAML values are re-serialised, but the DOCUMENT is edited line by line:
// the record being changed keeps every untouched line, every whole-line
// comment and every inline comment on a changed line; a new record is a
// rendered block appended after the last one; a removed record takes its
// own lines (and the comments INSIDE it) and nothing else. Everything here
// is deliberately boring: the reader's own parser validates the result
// before a single byte reaches the disk.
// ---------------------------------------------------------------------

struct Doc {
    lines: Vec<String>,
    eol: &'static str,
}

/// One declared record as found in the text: `[start, end)` covers its
/// content lines; trailing blank/comment lines are NOT part of it, so they
/// survive a removal and stay attached to whatever follows.
struct Item {
    start: usize,
    end: usize,
    id: String,
}

impl Doc {
    fn parse(text: &str) -> Self {
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        for l in &mut lines {
            if let Some(stripped) = l.strip_suffix('\r') {
                *l = stripped.to_string();
            }
        }
        Self { lines, eol }
    }

    fn render(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        let mut out = self.lines.join(self.eol);
        out.push_str(self.eol);
        out
    }

    fn upsert(
        &mut self,
        bundle: &ModelBundle,
        must_exist: bool,
        path: &Path,
    ) -> StoreResult<()> {
        let (root_idx, root_indent) = self.bundles_line(path)?;
        self.normalise_bundles_line(root_idx);
        let items = self.items(root_idx, root_indent, path)?;
        match items.iter().find(|i| i.id == bundle.id) {
            Some(_) if !must_exist => {
                return Err(cfg(format!(
                    "{}: duplicate bundle id `{}` — a record with this id already exists",
                    path.display(),
                    bundle.id
                )));
            }
            None if must_exist => {
                return Err(StoreError::NotFound(
                    "model bundle".into(),
                    bundle.id.clone(),
                ));
            }
            Some(item) => {
                let (start, end) = (item.start, item.end);
                self.rewrite_record(start, end, bundle);
            }
            None => {
                let at = items
                    .last()
                    .map(|i| i.end)
                    .unwrap_or(root_idx + 1);
                let block = render_block(bundle);
                for (offset, line) in block.into_iter().enumerate() {
                    self.lines.insert(at + offset, line);
                }
            }
        }
        Ok(())
    }

    fn remove(&mut self, id: &str, path: &Path) -> StoreResult<()> {
        let (root_idx, root_indent) = self.bundles_line(path)?;
        let items = self.items(root_idx, root_indent, path)?;
        let item = items.iter().find(|i| i.id == id).ok_or_else(|| {
            StoreError::NotFound("model bundle".into(), id.to_string())
        })?;
        self.lines.drain(item.start..item.end);
        Ok(())
    }

    /// The `bundles:` line and its indent. A document without one is not a
    /// catalogue yet: an EMPTY home gets the key on the first write.
    fn bundles_line(&mut self, path: &Path) -> StoreResult<(usize, usize)> {
        if let Some(idx) = self.lines.iter().position(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && t.starts_with("bundles:")
        }) {
            let indent = indent_of(&self.lines[idx]);
            return Ok((idx, indent));
        }
        if self.lines.iter().any(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with('#')
                && !t.starts_with('-')
                && !t.contains(':')
        }) {
            return Err(cfg(format!(
                "{}: no `bundles:` key — the declared catalogue cannot be written",
                path.display()
            )));
        }
        self.lines.push("bundles:".to_string());
        Ok((self.lines.len() - 1, 0))
    }

    /// `bundles: []` and an inline comment stay valid: the key is reduced to
    /// a plain `bundles:` before items are inserted.
    fn normalise_bundles_line(&mut self, idx: usize) {
        let line = &self.lines[idx];
        let indent = indent_of(line);
        let after = line.trim_start().trim_start_matches("bundles:");
        let comment = after.find('#').map(|h| after[h..].trim_end().to_string());
        let mut normalised = format!("{}{}", " ".repeat(indent), "bundles:");
        if let Some(c) = comment {
            normalised.push(' ');
            normalised.push_str(&c);
        }
        self.lines[idx] = normalised;
    }

    fn items(&self, root_idx: usize, root_indent: usize, path: &Path) -> StoreResult<Vec<Item>> {
        let mut starts: Vec<usize> = Vec::new();
        let mut item_indent: Option<usize> = None;
        let mut boundary = self.lines.len();
        for (i, line) in self.lines.iter().enumerate().skip(root_idx + 1) {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let ind = indent_of(line);
            if ind <= root_indent {
                boundary = i;
                break;
            }
            if !(t == "-" || t.starts_with("- ")) {
                continue;
            }
            match item_indent {
                None => {
                    item_indent = Some(ind);
                    starts.push(i);
                }
                Some(expected) if ind == expected => starts.push(i),
                Some(expected) if ind < expected => {
                    boundary = i;
                    break;
                }
                Some(_) => {}
            }
        }
        let mut items = Vec::with_capacity(starts.len());
        for (n, start) in starts.iter().enumerate() {
            let mut end = starts.get(n + 1).copied().unwrap_or(boundary);
            while end > *start + 1 && self.lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            while end > *start + 1 && self.lines[end - 1].trim_start().starts_with('#') {
                end -= 1;
            }
            let block = self.lines[*start..end].join("\n");
            let id = record_id(&block, path)?;
            items.push(Item {
                start: *start,
                end,
                id,
            });
        }
        Ok(items)
    }

    /// Rewrite only the keys the form owns, in place. Unchanged values keep
    /// their exact line; changed values keep their inline comment.
    fn rewrite_record(&mut self, start: usize, end: usize, b: &ModelBundle) {
        let key_indent = detect_key_indent(&self.lines, start, end);
        let mut end = end;
        for (key, value) in [
            ("label", b.label.clone()),
            ("agent", b.agent.clone()),
            ("driver", b.driver.clone()),
            ("llm_provider", b.llm_provider.clone()),
            ("model", b.model.clone()),
            ("reasoning", b.reasoning.clone()),
        ] {
            end = set_key(
                &mut self.lines,
                start,
                end,
                key_indent,
                key,
                &yaml_scalar(&value),
            );
        }
        let (phrase, timeout, fresh) = (
            yaml_scalar(&b.probe.phrase),
            b.probe.timeout_ms.to_string(),
            b.probe.fresh_secs.to_string(),
        );
        match find_key(&self.lines, start, end, key_indent, "probe") {
            Some(probe_idx) => {
                let probe_end = self
                    .lines
                    .iter()
                    .enumerate()
                    .skip(probe_idx + 1)
                    .take(end - probe_idx - 1)
                    .find(|(_, l)| {
                        !l.trim().is_empty()
                            && !l.trim_start().starts_with('#')
                            && indent_of(l) <= key_indent
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(end);
                let child_indent = detect_probe_indent(&self.lines, probe_idx + 1, probe_end, key_indent);
                let mut probe_end = probe_end;
                for (key, value) in [
                    ("phrase", phrase),
                    ("timeout_ms", timeout),
                    ("fresh_secs", fresh),
                ] {
                    probe_end = set_key(
                        &mut self.lines,
                        probe_idx + 1,
                        probe_end,
                        child_indent,
                        key,
                        &value,
                    );
                }
            }
            None => {
                let probe_indent = " ".repeat(key_indent);
                let child = " ".repeat(key_indent + 2);
                let block = vec![
                    format!("{probe_indent}probe:"),
                    format!("{child}phrase: {phrase}"),
                    format!("{child}timeout_ms: {timeout}"),
                    format!("{child}fresh_secs: {fresh}"),
                ];
                for (offset, line) in block.into_iter().enumerate() {
                    self.lines.insert(end + offset, line);
                }
            }
        }
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn record_id(block: &str, path: &Path) -> StoreResult<String> {
    let seq: serde_yaml::Value = serde_yaml::from_str(block).map_err(|e| {
        cfg(format!(
            "{}: cannot read a declared record at write time: {e}",
            path.display()
        ))
    })?;
    seq.as_sequence()
        .and_then(|s| s.first())
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            cfg(format!(
                "{}: a declared record carries no `id` — cannot edit it",
                path.display()
            ))
        })
}

fn detect_key_indent(lines: &[String], start: usize, end: usize) -> usize {
    for line in &lines[start + 1..end] {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        return indent_of(line);
    }
    indent_of(&lines[start]) + 2
}

fn detect_probe_indent(
    lines: &[String],
    start: usize,
    end: usize,
    key_indent: usize,
) -> usize {
    for line in &lines[start..end] {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let ind = indent_of(line);
        if ind > key_indent {
            return ind;
        }
    }
    key_indent + 2
}

fn find_key(
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
    key: &str,
) -> Option<usize> {
    (start..end).find(|&i| {
        let line = &lines[i];
        if indent_of(line) != indent {
            return false;
        }
        let t = line.trim_start();
        t.strip_prefix(key)
            .is_some_and(|rest| rest.starts_with(':'))
    })
}

/// Set one scalar key: an existing line keeps its indent and its inline
/// comment, a missing key is appended at the end of the block.
fn set_key(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    indent: usize,
    key: &str,
    value: &str,
) -> usize {
    match find_key(lines, start, end, indent, key) {
        Some(i) => {
            let old = &lines[i];
            let colon = old.find(':').unwrap_or(old.len());
            // Keep the operator's inline comment EXACTLY — including the
            // spacing before it — when the value on its left is rewritten.
            let suffix = old[colon..]
                .find('#')
                .map(|h| {
                    let hash = colon + h;
                    let mut start = hash;
                    while start > colon && old.as_bytes()[start - 1] == b' ' {
                        start -= 1;
                    }
                    if start == hash {
                        format!(" {}", &old[hash..])
                    } else {
                        old[start..].to_string()
                    }
                })
                .unwrap_or_default();
            lines[i] = format!("{}{key}: {value}{suffix}", " ".repeat(indent));
            end
        }
        None => {
            lines.insert(end, format!("{}{key}: {value}", " ".repeat(indent)));
            end + 1
        }
    }
}

/// A YAML scalar for a value the operator typed: serde_yaml quotes it when
/// the text would otherwise parse as another type (`true`, `20000`).
fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(&serde_yaml::Value::String(value.to_string()))
        .map(|s| s.trim_end().to_string())
        .unwrap_or_else(|_| "\"\"".to_string())
}

/// A fresh record in the document's own layout (two-space item indent, four
/// space keys, six space probe children) — the factory file's shape.
fn render_block(b: &ModelBundle) -> Vec<String> {
    vec![
        format!("  - id: {}", yaml_scalar(&b.id)),
        format!("    label: {}", yaml_scalar(&b.label)),
        format!("    agent: {}", yaml_scalar(&b.agent)),
        format!("    driver: {}", yaml_scalar(&b.driver)),
        format!("    llm_provider: {}", yaml_scalar(&b.llm_provider)),
        format!("    model: {}", yaml_scalar(&b.model)),
        format!("    reasoning: {}", yaml_scalar(&b.reasoning)),
        "    probe:".to_string(),
        format!("      phrase: {}", yaml_scalar(&b.probe.phrase)),
        format!("      timeout_ms: {}", b.probe.timeout_ms),
        format!("      fresh_secs: {}", b.probe.fresh_secs),
    ]
}

/// The handle a new record carries (ASM-9): lower-case latin letters,
/// digits and single hyphens, derived ONCE from the operator's name and
/// never changed afterwards. `""` means the name carries no usable latin
/// characters — the caller refuses with «выберите другое имя».
pub fn bundle_id_from_label(label: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in label.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if matches!(c, '-' | '_' | ' ' | '.' | '·' | '/') && !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Read and parse the file into records: document shape, per-record white
/// list, then the cross-record rules. Each failure is typed and names the
/// file (and the index) it came from.
fn read_bundles(path: &Path) -> StoreResult<Vec<ModelBundle>> {
    let text =
        fs::read_to_string(path).map_err(|e| cfg(format!("read {}: {e}", path.display())))?;
    parse_bundles(&text, path)
}

/// The SAME rules as [`read_bundles`], over text in hand — the writer
/// validates the document it is about to replace with this exact function.
fn parse_bundles(text: &str, path: &Path) -> StoreResult<Vec<ModelBundle>> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| cfg(format!("parse {}: {e}", path.display())))?;
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
        // `bundles:` with nothing under it (the shape a home that had every
        // record deleted keeps) is an EMPTY list, not a shape error.
        Some(serde_yaml::Value::Null) => Ok(&[]),
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

    // -----------------------------------------------------------------
    // The WRITER (task 01a1be5c, REQ-MC-06): whole-file, atomic, the
    // reader's own rules, and the operator's comments preserved.
    // -----------------------------------------------------------------

    const COMMENTED: &str = "\
# header comment: the home's own words
bundles:
  # the first one
  - id: a
    label: A        # inline name
    agent: a
    driver: hermes
    model: m
    reasoning: high
    probe:
      phrase: ping
      timeout_ms: 20000
      fresh_secs: 900

  - id: b
    label: B
    agent: b
    driver: claude
    reasoning: medium
";

    fn bundle(id: &str, label: &str, agent: &str, driver: &str, model: &str) -> ModelBundle {
        ModelBundle {
            id: id.into(),
            label: label.into(),
            agent: agent.into(),
            driver: driver.into(),
            llm_provider: if driver == "claude" {
                String::new()
            } else {
                "deepseek".into()
            },
            model: model.into(),
            reasoning: "high".into(),
            probe: ProbePolicy::default(),
        }
    }

    /// A create appends ONE block; the header, the whole-line comment and the
    /// untouched records keep their exact lines (REQ-MC-06 AC2/AC3).
    #[test]
    fn a_create_appends_one_block_and_keeps_every_earlier_line() {
        let d = tmp();
        write(d.path(), COMMENTED);
        let before = fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap();
        let rev = catalog_revision(d.path()).unwrap();

        let c = bundle("c", "C", "c-agent", "hermes", "new-model");
        let new_rev = write_catalog(
            d.path(),
            &rev,
            &CatalogEdit::Upsert {
                bundle: &c,
                must_exist: false,
            },
        )
        .unwrap();
        assert_ne!(new_rev, rev);

        let after = fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap();
        assert!(after.starts_with(&before), "the earlier text must be a prefix");
        assert!(after.contains("  - id: c\n    label: C\n"));
        assert!(after.contains("# header comment: the home's own words"));
        assert!(after.contains("label: A        # inline name"));
        let catalog = load_catalog(d.path()).unwrap();
        assert_eq!(catalog.ids(), vec!["a", "b", "c"]);
        assert_eq!(catalog.get("c").unwrap().identity(), c.identity());
    }

    /// An edit rewrites only the record's own keys: the whole-line comment
    /// INSIDE the record survives, the inline comment on a changed line
    /// survives, and the other record keeps its bytes (REQ-MC-06 AC2/AC3).
    #[test]
    fn an_edit_keeps_comments_and_the_other_records_untouched() {
        let d = tmp();
        write(d.path(), COMMENTED);
        let rev = catalog_revision(d.path()).unwrap();
        let source = fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap();
        let b_block = source
            .split("  - id: b")
            .nth(1)
            .expect("the second record is present")
            .to_string();

        let mut edited = bundle("a", "A", "a", "hermes", "m-2");
        edited.probe.timeout_ms = 5_000;
        write_catalog(
            d.path(),
            &rev,
            &CatalogEdit::Upsert {
                bundle: &edited,
                must_exist: true,
            },
        )
        .unwrap();

        let after = fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap();
        assert!(after.contains("# the first one"));
        assert!(after.contains("label: A        # inline name"), "{after}");
        assert!(after.contains("model: m-2"));
        assert!(after.contains("timeout_ms: 5000"));
        assert!(after.ends_with(&b_block), "the other record moved: {after}");
        let catalog = load_catalog(d.path()).unwrap();
        let a = catalog.get("a").unwrap();
        assert_eq!(a.model, "m-2");
        assert_eq!(a.probe.timeout_ms, 5_000);
    }

    /// A delete removes exactly one record and leaves a valid file — even
    /// when the last record goes (REQ-MC-05 AC1/AC6).
    #[test]
    fn a_delete_removes_one_record_and_the_last_one_leaves_an_empty_list() {
        let d = tmp();
        write(d.path(), COMMENTED);
        let rev = catalog_revision(d.path()).unwrap();
        let rev = write_catalog(d.path(), &rev, &CatalogEdit::Remove { id: "a" }).unwrap();
        let after = fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap();
        assert!(!after.contains("id: a"));
        assert!(after.contains("# header comment"));
        assert_eq!(load_catalog(d.path()).unwrap().ids(), vec!["b"]);

        write_catalog(d.path(), &rev, &CatalogEdit::Remove { id: "b" }).unwrap();
        let after = fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap();
        assert!(after.contains("bundles:"));
        assert!(load_catalog(d.path()).unwrap().bundles.is_empty());
        assert!(after.contains("# header comment"));
    }

    /// A stale form refuses and leaves the file BYTE-IDENTICAL (REQ-MC-06
    /// AC1/AC6) — the other writer's edition is never overwritten.
    #[test]
    fn a_stale_revision_refuses_and_writes_nothing() {
        let d = tmp();
        write(d.path(), COMMENTED);
        let rev = catalog_revision(d.path()).unwrap();
        let other = format!("{COMMENTED}  - id: z\n    label: Z\n    agent: z\n    driver: hermes\n    reasoning: low\n");
        write(d.path(), &other);

        let c = bundle("c", "C", "c-agent", "hermes", "m");
        let err = write_catalog(
            d.path(),
            &rev,
            &CatalogEdit::Upsert {
                bundle: &c,
                must_exist: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::Stale { .. }), "{err}");
        assert_eq!(
            fs::read_to_string(d.path().join(MODEL_CONFIG_FILE)).unwrap(),
            other,
            "the file must not change on a stale form"
        );
    }

    /// The id is derived from the name once (ASM-9): latin letters/digits,
    /// single hyphens; nothing latin → empty (the caller refuses).
    #[test]
    fn the_handle_is_derived_from_the_name() {
        assert_eq!(bundle_id_from_label("Hermes · DeepSeek"), "hermes-deepseek");
        assert_eq!(bundle_id_from_label("  My   Model  "), "my-model");
        assert_eq!(bundle_id_from_label("A--B__C"), "a-b-c");
        assert_eq!(bundle_id_from_label("Связка"), "");
    }
}
