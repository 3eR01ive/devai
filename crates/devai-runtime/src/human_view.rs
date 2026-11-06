//! The HUMAN FORM of a quality report (REQ-QH-01..04): a structured,
//! stored-at-publication presentation layer, never re-derived at read time
//! (no-read-time-rederivation).
//!
//! Assembly (`build`) runs exactly once, inside `evaluation::assemble`, when
//! (id, score, weight, floor, decided-at-assembly flags) are already in hand.
//! Every reader — the CLI gates, the UI report panel — quotes the stored
//! struct verbatim; `render_text` is a pure layout of stored fields, it
//! computes no importance, no group membership and no ordering.
//!
//! Grouping is by DECISION WEIGHT, not by check id:
//!   1. blocks    — «бьёт по вердикту / требует твоего решения» (gates,
//!                  floor breaches, mechanical fails, pending human signature);
//!   2. nuances   — every remaining scored point with score < 1.0 (ratified
//!                  OQ-QH-1: no 0.8–0.9 band filter; 0.4 without a breach is
//!                  still the human's business), ordered by lost-points×weight;
//!   3. anchors   — positions with an EXPLICIT ratified_by link to a stored
//!                  decision; names only, junk-drawer forbidden (OQ-QH-2);
//!   4. unchecked — BLOCKED checks with their reason and weight.
//!
//! Code and probe output live in the per-item `evidence` layer (REQ-QH-04),
//! never inside the visible one-line summary.

use std::collections::BTreeMap;

use devai_store::config::Profile;
use serde::{Deserialize, Serialize};

use crate::evaluation::{CheckOutcome, GateOutcome, ProbeState};

/// Version of the presented shape itself (the report contract versions the
/// document; this versions the human form inside it).
pub const HUMAN_VIEW_VERSION: u32 = 1;

pub const GROUP_BLOCKS: &str = "blocks";
pub const GROUP_NUANCES: &str = "nuances";
pub const GROUP_ANCHORS: &str = "anchors";
pub const GROUP_UNCHECKED: &str = "unchecked";

pub const TITLE_BLOCKS: &str = "БЬЁТ ПО ВЕРДИКТУ / ТРЕБУЕТ ТВОЕГО РЕШЕНИЯ";
pub const TITLE_NUANCES: &str = "НЮАНСЫ";
pub const TITLE_ANCHORS: &str = "ЯКОРЯ / РАТИФИЦИРОВАНО";
pub const TITLE_UNCHECKED: &str = "НЕ ПРОВЕРЕНО / ЖДЁТ";

/// Summary lines never carry code: this is the hard cap of the visible line.
const SUMMARY_MAX: usize = 160;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HumanReportView {
    /// The stored shape version (`HUMAN_VIEW_VERSION` at assembly time).
    pub version: u32,
    /// Language of every human string in this shape (the task language, ru).
    pub lang: String,
    pub headline: HumanHeadline,
    /// Exactly the four groups, in this fixed order. An empty group is
    /// still present here as data — the RENDERER omits its heading (the
    /// conditional-section precedent of §2.6 `render`).
    pub groups: Vec<HumanGroup>,
    /// Blocked weight as decided at assembly, so the «не проверено» heading
    /// needs no recomputation by any reader.
    pub blocked_weight: f64,
    pub max_blocked_weight: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HumanHeadline {
    /// Human verdict word: ПРОШЁЛ / НЕ ПРОШЁЛ / ВЕРДИКТ НЕ ГОТОВ.
    pub verdict: String,
    /// Machine verdict: PASS / FAIL / INCONCLUSIVE.
    pub machine_verdict: String,
    pub score: f64,
    pub theta: f64,
    pub basis: String,
    /// Counts per group — the «what hits the verdict, what is noise» line.
    pub counts: HumanCounts,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct HumanCounts {
    pub blocks: usize,
    pub nuances: usize,
    pub anchors: usize,
    pub unchecked: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HumanGroup {
    /// blocks | nuances | anchors | unchecked
    pub key: String,
    pub title: String,
    /// One line of orientation per group; empty when the group speaks for
    /// itself (anchors).
    pub lead: String,
    pub items: Vec<HumanItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HumanItem {
    /// Check id or gate id — the identity the operator cites back.
    pub id: String,
    /// Why this item stands where it stands: gate | pending | floor |
    /// mechanical | scored | blocked | anchor.
    pub kind: String,
    /// ONE short visible line: what happened, no code, no paragraphs.
    pub summary: String,
    /// file:line of the finding, empty when the item has no site.
    pub site: String,
    pub score: Option<f64>,
    pub weight: Option<f64>,
    /// Lost points × weight, decided at assembly (REQ-QH-02). `None` for
    /// items without a score (pending, blocked, gates) — the ratified
    /// assumption: their importance is not a number, they are not noise.
    pub importance: Option<f64>,
    /// The stored decision that keeps this anchor in force (OQ-QH-2);
    /// `None` for every non-anchor item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratified_by: Option<String>,
    /// BLOCKED reason / gate detail — a fact, not a judgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The collapsed proof layer (REQ-QH-04): citation bodies, probe
    /// output, violation paragraphs — preserved verbatim as stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// Build the human form from ALREADY DECIDED values. Deterministic: same
/// inputs, same bytes — every ordering below is a total order with an id
/// tiebreak, and no map iteration order leaks in.
#[allow(clippy::too_many_arguments)]
pub fn build(
    profile: &Profile,
    machine_verdict: &str,
    score: f64,
    theta: f64,
    basis: &str,
    checks: &[CheckOutcome],
    gates: &[GateOutcome],
    floors_breached: &[String],
    pending: &[String],
    blocked_weight: f64,
    max_blocked_weight: f64,
    ratified: &BTreeMap<String, Ratified>,
) -> HumanReportView {
    let statements: BTreeMap<&str, &str> = profile
        .checks()
        .into_iter()
        .map(|c| (c.id.as_str(), c.statement.as_str()))
        .collect();
    // Breached ids are parsed from the decided-at-assembly strings
    // ("id=0.40<floor 0.70") — the flag itself is never re-derived here.
    let breached: Vec<&str> = floors_breached
        .iter()
        .filter_map(|s| s.split('=').next())
        .collect();

    let mut blocks: Vec<(u8, Option<f64>, HumanItem)> = Vec::new();
    let mut nuances: Vec<(Option<f64>, HumanItem)> = Vec::new();
    let mut anchors: Vec<HumanItem> = Vec::new();
    let mut unchecked: Vec<(f64, HumanItem)> = Vec::new();

    // «Блокирующее сверх кэпа» (REQ-QH-03): when the weight of unperformed
    // checks alone can hold the verdict hostage, that is a decision item, not
    // a footnote — the headline basis says it too, the group shows it as a
    // row. The numbers are the decided-at-assembly ones.
    if blocked_weight > max_blocked_weight {
        blocks.push((
            0,
            None,
            HumanItem {
                id: "blocked-weight".into(),
                kind: "cap".into(),
                summary: format!(
                    "невыполненных проверок на вес {blocked_weight:.2} > {max_blocked_weight:.2} — вердикт не может быть готовым"
                ),
                site: String::new(),
                score: None,
                weight: Some(blocked_weight),
                importance: None,
                ratified_by: None,
                reason: None,
                evidence: Vec::new(),
            },
        ));
    }

    // Gates first: a failed gate is the decision, not a nuance.
    for g in gates.iter().filter(|g| g.state != ProbeState::Pass) {
        blocks.push((
            0,
            None,
            HumanItem {
                id: g.id.clone(),
                kind: if g.state == ProbeState::Fail {
                    "gate".into()
                } else {
                    "gate-unperformed".into()
                },
                summary: match g.state {
                    ProbeState::Fail => "шлюз не пройден — вердикт возвращается".to_string(),
                    _ => "шлюз не выполнен — вердикт не может быть окончательным".to_string(),
                },
                site: first_site(&g.detail).unwrap_or_default(),
                score: None,
                weight: None,
                importance: None,
                ratified_by: None,
                reason: Some(g.detail.clone()).filter(|d| !d.trim().is_empty()),
                evidence: Vec::new(),
            },
        ));
    }

    for c in checks {
        let statement = statements.get(c.id.as_str()).copied().unwrap_or("");
        let rat = ratified.get(&c.id);
        match c.state.as_str() {
            "PENDING_HUMAN" => {
                let item = HumanItem {
                    id: c.id.clone(),
                    kind: "pending".into(),
                    summary: one_line(statement, "нужна твоя подпись — вердикт не готов"),
                    site: String::new(),
                    score: None,
                    weight: Some(c.weight),
                    importance: None,
                    ratified_by: None,
                    reason: pending.iter().find(|p| **p == c.id).map(|p| {
                        format!("{p}: подпись человека, черновик балла не финализируется")
                    }),
                    evidence: c.evidence.clone(),
                };
                blocks.push((1, None, item));
            }
            "BLOCKED" => {
                unchecked.push((
                    c.weight,
                    HumanItem {
                        id: c.id.clone(),
                        kind: "blocked".into(),
                        summary: one_line(statement, "проверка не выполнена — балла нет"),
                        site: String::new(),
                        score: None,
                        weight: Some(c.weight),
                        importance: None,
                        ratified_by: rat.map(|r| r.display()),
                        reason: Some(c.reason.clone()).filter(|r| !r.trim().is_empty()),
                        evidence: c.evidence.clone(),
                    },
                ));
            }
            _ => {
                let score = c.score;
                let is_breach = breached.iter().any(|b| *b == c.id);
                let is_mech_fail = c.state == "MECHANICAL_FAIL";
                let lost = score.map(|s| 1.0 - s);
                let importance = match (lost, score) {
                    (Some(l), Some(_)) if l > 1e-9 => Some(l * c.weight),
                    _ => None,
                };
                let summary = visible_summary(c, statement);
                let site = visible_site(c);
                // Anchors: an explicit ratified_by link and NOTHING to fix
                // (OQ-QH-2 — a breached anchored check is a decision item,
                // never filed away as a name).
                if let Some(r) = rat {
                    if importance.is_none() && !is_breach && !is_mech_fail {
                        anchors.push(HumanItem {
                            id: c.id.clone(),
                            kind: "anchor".into(),
                            summary: if statement.is_empty() {
                                "закреплено решением".into()
                            } else {
                                one_line(statement, "закреплено решением")
                            },
                            site: String::new(),
                            score,
                            weight: Some(c.weight),
                            importance: None,
                            ratified_by: Some(r.display()),
                            reason: None,
                            evidence: Vec::new(),
                        });
                        continue;
                    }
                }
                let item = HumanItem {
                    id: c.id.clone(),
                    kind: if is_breach {
                        "floor".into()
                    } else if is_mech_fail {
                        "mechanical".into()
                    } else {
                        "scored".into()
                    },
                    summary,
                    site,
                    score,
                    weight: Some(c.weight),
                    importance,
                    ratified_by: rat.map(|r| r.display()),
                    reason: None,
                    evidence: collapsed_layer(c),
                };
                if is_breach || is_mech_fail {
                    // Floor breach = «нарушенный порог», mechanical FAIL =
                    // «блокирующее»: both land above every nuance, in the
                    // operator's decision block.
                    blocks.push((if is_breach { 2 } else { 3 }, importance, item));
                } else if importance.is_some() {
                    nuances.push((importance, item));
                }
                // A perfect, unanchored point is not presented: nothing to
                // decide, and the human paragraph of the verdict carries S.
            }
        }
    }

    // Totals: importance desc, then id — a total order, byte-stable.
    blocks.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| desc(a.1, b.1))
            .then_with(|| a.2.id.cmp(&b.2.id))
    });
    nuances.sort_by(|a, b| desc(a.0, b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    unchecked.sort_by(|a, b| desc(Some(a.0), Some(b.0)).then_with(|| a.1.id.cmp(&b.1.id)));
    anchors.sort_by(|a, b| a.id.cmp(&b.id));

    let groups = vec![
        HumanGroup {
            key: GROUP_BLOCKS.into(),
            title: TITLE_BLOCKS.into(),
            lead:
                "их единицы: нарушенные пороги, проваленные шлюзы, блокирующее и места под подпись"
                    .into(),
            items: blocks.into_iter().map(|(_, _, i)| i).collect(),
        },
        HumanGroup {
            key: GROUP_NUANCES.into(),
            title: TITLE_NUANCES.into(),
            lead: "построчно: проверка, балл, вес, важность, сайт, суть".into(),
            items: nuances.into_iter().map(|(_, i)| i).collect(),
        },
        HumanGroup {
            key: GROUP_ANCHORS.into(),
            title: TITLE_ANCHORS.into(),
            lead: String::new(),
            items: anchors,
        },
        HumanGroup {
            key: GROUP_UNCHECKED.into(),
            title: TITLE_UNCHECKED.into(),
            lead: String::new(),
            items: unchecked.into_iter().map(|(_, i)| i).collect(),
        },
    ];
    let counts = HumanCounts {
        blocks: groups[0].items.len(),
        nuances: groups[1].items.len(),
        anchors: groups[2].items.len(),
        unchecked: groups[3].items.len(),
    };

    HumanReportView {
        version: HUMAN_VIEW_VERSION,
        lang: "ru".into(),
        headline: HumanHeadline {
            verdict: match machine_verdict {
                "PASS" => "ПРОШЁЛ",
                "FAIL" => "НЕ ПРОШЁЛ",
                _ => "ВЕРДИКТ НЕ ГОТОВ",
            }
            .into(),
            machine_verdict: machine_verdict.into(),
            score,
            theta,
            basis: basis.into(),
            counts,
        },
        groups,
        blocked_weight,
        max_blocked_weight,
    }
}

/// A ratified-by reference as printed to the human: the decision id plus,
/// when the store knows it, the actor who ratified it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ratified {
    pub decision_id: String,
    pub actor: String,
}

impl Ratified {
    pub fn display(&self) -> String {
        if self.actor.trim().is_empty() {
            format!("решение {}", self.decision_id)
        } else {
            format!("решение {} ({})", self.decision_id, self.actor)
        }
    }
}

fn desc(a: Option<f64>, b: Option<f64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// The visible one-line essence of a scored point: the auditor's first
/// finding sentence when there is one, else the check statement. Code-shaped
/// lines are skipped and inline code spans are flattened — REQ-QH-04: no
/// rendered code inside a visible line.
fn visible_summary(c: &CheckOutcome, statement: &str) -> String {
    for v in &c.violations {
        if let Some(line) = first_prose_line(v) {
            return line;
        }
    }
    one_line(statement, "см. доказательства")
}

fn visible_site(c: &CheckOutcome) -> String {
    for v in &c.violations {
        if let Some(s) = first_site(v) {
            return s;
        }
    }
    for e in &c.evidence {
        if let Some(s) = first_site(e) {
            return s;
        }
    }
    String::new()
}

/// The collapsed proof layer of one point: the finding bodies (exactly what
/// §2.6 used to inline into the paragraph) followed by the check's own
/// evidence. Nothing is dropped (REQ-QH-04: evidence changes LAYER, never
/// disappears); duplicates are collapsed only when byte-identical.
fn collapsed_layer(c: &CheckOutcome) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in &c.violations {
        if !v.trim().is_empty() && !out.iter().any(|x| x == v) {
            out.push(v.clone());
        }
    }
    for e in &c.evidence {
        if !e.trim().is_empty() && !out.iter().any(|x| x == e) {
            out.push(e.clone());
        }
    }
    out
}

/// First line that reads as prose (not a code block, not a shell/probe
/// line), flattened of inline code markers and capped at `SUMMARY_MAX`.
fn first_prose_line(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || is_code_line(line) {
            continue;
        }
        return Some(one_line(line, ""));
    }
    None
}

fn is_code_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('+') && !t.starts_with("+ ") {
        // a diff-added source line
        return true;
    }
    [
        "```",
        "~~~",
        "|",
        "diff --git",
        "$ ",
        ">>> ",
        "grep ",
        "rg ",
        "cargo ",
        "npm ",
        "fn ",
        "let ",
        "pub fn ",
        "def ",
        "class ",
        "#[",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

/// `text` flattened to one line, inline `code` markers removed, capped —
/// never empty (falls back to `alt`).
fn one_line(text: &str, alt: &str) -> String {
    let flat: String = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let flat = flat.trim_matches('"').trim().to_string();
    if flat.is_empty() {
        return alt.to_string();
    }
    if flat.chars().count() > SUMMARY_MAX {
        let cut: String = flat.chars().take(SUMMARY_MAX).collect();
        format!("{}…", cut.trim_end())
    } else {
        flat
    }
}

/// The first `path:line` token in a text (suffix whitelist keeps prose like
/// «шаг 2» out of the site column).
fn first_site(text: &str) -> Option<String> {
    const EXTS: &[&str] = &[
        "rs", "ts", "tsx", "js", "vue", "py", "md", "toml", "yaml", "yml", "json", "sh", "sql",
        "go", "java", "c", "cpp", "h",
    ];
    for tok in
        text.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '(' || c == ')')
    {
        let tok = tok.trim_matches(|c: char| {
            matches!(c, '`' | '"' | '\'' | '[' | ']' | ':' | '.' | '(' | ')')
        });
        let Some((path, line)) = tok.rsplit_once(':') else {
            continue;
        };
        if line.is_empty() || !line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Some((_, ext)) = path.rsplit_once('.') else {
            continue;
        };
        if !EXTS.contains(&ext) || path.chars().any(|c| c.is_whitespace()) {
            continue;
        }
        return Some(format!("{path}:{line}"));
    }
    None
}

/// Lay the stored form out as text. PURE LAYOUT: every number, group and
/// order below is read from the struct — nothing is recomputed here, so the
/// CLI text of one stored form is byte-identical on every read.
pub fn render_text(view: &HumanReportView) -> String {
    let h = &view.headline;
    let mut out: Vec<String> = Vec::new();
    out.push("ОТЧЁТ КАЧЕСТВА · человеческая форма".to_string());
    out.push(String::new());
    out.push(format!(
        "ВЕРДИКТ: {} ({}) — S={:.3}/1.00, порог {:.2}: {}",
        h.verdict, h.machine_verdict, h.score, h.theta, h.basis
    ));
    out.push(format!(
        "бьёт по вердикту: {} · нюансы: {} · якоря: {} · не проверено: {}",
        h.counts.blocks, h.counts.nuances, h.counts.anchors, h.counts.unchecked
    ));
    for (gi, g) in view.groups.iter().enumerate() {
        if g.items.is_empty() {
            // The conditional-section precedent: an empty group prints no
            // heading at all.
            continue;
        }
        out.push(String::new());
        out.push(format!("{}) {} ({})", gi + 1, g.title, g.items.len()));
        if !g.lead.is_empty() {
            out.push(format!("   {}", g.lead));
        }
        for (ii, it) in g.items.iter().enumerate() {
            if g.key == GROUP_ANCHORS {
                out.push(format!(
                    "  · {} — закреплено: {}",
                    it.id,
                    it.ratified_by.as_deref().unwrap_or("—")
                ));
                continue;
            }
            out.push(format!("  {}. {}", ii + 1, item_headline(it)));
            if !it.summary.is_empty() {
                out.push(format!("     суть: {}", it.summary));
            }
            if let Some(reason) = &it.reason {
                out.push(format!("     причина: {}", reason));
            }
            if !it.evidence.is_empty() {
                out.push(format!(
                    "     └ доказательства (свёрнуто, {}):",
                    it.evidence.len()
                ));
                for e in &it.evidence {
                    for line in e.lines() {
                        out.push(format!("       │ {}", line.trim_end()));
                    }
                }
            }
        }
    }
    out.join("\n")
}

fn item_headline(it: &HumanItem) -> String {
    let mut parts: Vec<String> = vec![it.id.clone()];
    let n = |v: f64| format!("{v:.2}");
    match it.score {
        Some(s) => parts.push(format!("балл {}", n(s))),
        None => parts.push("балл —".into()),
    }
    if let Some(w) = it.weight {
        parts.push(format!("вес {}", n(w)));
    }
    if let Some(i) = it.importance {
        parts.push(format!("важность {}", format!("{i:.3}")));
    }
    if !it.site.is_empty() {
        parts.push(it.site.clone());
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_store::config::Profile;

    /// A profile whose weights/floors make the ordering expectations exact:
    /// `blk/floor` detects a catastrophic type (floor 0.70), the two nuance
    /// points stay above their minor floor so they are NOT decision items.
    fn profile() -> Profile {
        let yaml = r#"
version: 1
project: formtest
applies_to_roles: [quality-auditor]
error_types:
  correctness/state-transition: { desc: d, severity: catastrophic }
  process/untested-branch: { desc: d, severity: minor }
gates:
  - id: gate-1
    desc: g
    probes: [{ cmd: "true", pass_exit: [0] }]
evaluations:
  - id: correctness
    title: T
    checks:
      - id: blk/floor
        statement: state transition is checked
        detects: [correctness/state-transition]
        verify: rubric
        anchors: { "1.0": ok, "0.4": mid, "0.0": bad }
        reviewer: either
      - id: n/big
        statement: big nuance
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: either
      - id: n/small
        statement: small nuance
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: either
      - id: ok/full
        statement: nothing to fix here
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: either
      - id: anch/a
        statement: ratified invariant
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: either
      - id: brk/x
        statement: unperformed
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: either
      - id: pend/p
        statement: needs your signature
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: human
scoring:
  theta: 0.8
  floors: { catastrophic: 0.7, major: 0.4, minor: null }
  blocked_policy: { max_blocked_weight: 0.2 }
  weights:
    blk/floor: 0.20
    n/big: 0.50
    n/small: 0.10
    ok/full: 0.00
    anch/a: 0.00
    brk/x: 0.05
    pend/p: 0.15
"#;
        serde_yaml::from_str(yaml).expect("form fixture profile parses")
    }

    fn check(id: &str, state: &str, score: Option<f64>, weight: f64) -> CheckOutcome {
        CheckOutcome {
            id: id.into(),
            verify: "rubric".into(),
            state: state.into(),
            score,
            floor: None,
            weight,
            detects: vec![],
            violations: vec![],
            evidence: vec![format!("evidence for {id}")],
            reason: String::new(),
        }
    }

    fn ratified_map() -> BTreeMap<String, Ratified> {
        let mut m = BTreeMap::new();
        m.insert(
            "anch/a".to_string(),
            Ratified {
                decision_id: "01a1b1b8-dec".into(),
                actor: "guerrero".into(),
            },
        );
        m
    }

    fn fixture() -> HumanReportView {
        let p = profile();
        let mut floor = check("blk/floor", "SCORED", Some(0.4), 0.20);
        floor.floor = Some(0.7);
        floor.violations = vec![
            "state transition is not guarded\n```\nfn step() { unchecked() }\n```\nthe guard is missing at src/engine.rs:412".to_string(),
        ];
        let mut big = check("n/big", "SCORED", Some(0.3), 0.50);
        big.violations = vec!["cargo test prints a stack: see src/a.rs:10".into()];
        let small = check("n/small", "SCORED", Some(0.9), 0.10);
        let full = check("ok/full", "SCORED", Some(1.0), 0.00);
        let anchor = check("anch/a", "SCORED", Some(1.0), 0.00);
        let mut blocked = check("brk/x", "BLOCKED", None, 0.05);
        blocked.reason = "probe could not run".into();
        let pending = check("pend/p", "PENDING_HUMAN", None, 0.15);
        let checks = vec![floor, big, small, full, anchor, blocked, pending];
        let thresholds = vec!["blk/floor=0.40<floor 0.70".to_string()];
        build(
            &p,
            "FAIL",
            0.55,
            0.8,
            "красные линии: blk/floor=0.40<floor 0.70",
            &checks,
            &[],
            &thresholds,
            &["pend/p".to_string()],
            0.05,
            0.2,
            &ratified_map(),
        )
    }

    #[test]
    fn every_point_carries_score_weight_and_importance_in_decision_order() {
        let v = fixture();
        assert_eq!(v.version, HUMAN_VIEW_VERSION);
        assert_eq!(v.lang, "ru");
        let keys: Vec<&str> = v.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![GROUP_BLOCKS, GROUP_NUANCES, GROUP_ANCHORS, GROUP_UNCHECKED],
            "four groups in the fixed order"
        );
        assert_eq!(v.headline.verdict, "НЕ ПРОШЁЛ");
        assert_eq!(v.headline.counts.blocks, 2);

        // Blocks: the unsigned human point and the breached floor, no noise.
        let blocks: Vec<&str> = v.groups[0].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(blocks, vec!["pend/p", "blk/floor"]);
        let floor = &v.groups[0].items[1];
        assert_eq!(floor.kind, "floor");
        assert_eq!(floor.score, Some(0.4));
        assert_eq!(floor.weight, Some(0.20));
        // importance = (1-0.4)*0.20
        assert!((floor.importance.unwrap() - 0.12).abs() < 1e-9);

        // Nuances: EVERY remaining scored<1.0 (0.3 included — ratified
        // OQ-QH-1 has no 0.8-0.9 band), most lost-points×weight first.
        let nuances: Vec<&str> = v.groups[1].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(nuances, vec!["n/big", "n/small"]);
        assert!((v.groups[1].items[0].importance.unwrap() - 0.35).abs() < 1e-9);
        assert!((v.groups[1].items[1].importance.unwrap() - 0.01).abs() < 1e-9);
        for it in &v.groups[1].items {
            assert!(it.score.is_some() && it.weight.is_some() && it.importance.is_some());
        }

        // Anchors: names only, and only with an explicit ratified_by link.
        assert_eq!(v.groups[2].items.len(), 1);
        assert_eq!(v.groups[2].items[0].id, "anch/a");
        assert_eq!(
            v.groups[2].items[0].ratified_by.as_deref(),
            Some("решение 01a1b1b8-dec (guerrero)")
        );
        assert!(
            v.groups[2].items[0].evidence.is_empty(),
            "anchor = name only"
        );

        // Unchecked: the BLOCKED check with its reason and weight.
        assert_eq!(v.groups[3].items.len(), 1);
        assert_eq!(v.groups[3].items[0].id, "brk/x");
        assert_eq!(v.groups[3].items[0].weight, Some(0.05));
        assert_eq!(
            v.groups[3].items[0].reason.as_deref(),
            Some("probe could not run")
        );
        // The perfect, unanchored point is not presented at all.
        assert!(v
            .groups
            .iter()
            .all(|g| g.items.iter().all(|i| i.id != "ok/full")));
    }

    #[test]
    fn blocked_weight_over_cap_is_a_decision_row() {
        let p = profile();
        let blocked = check("brk/x", "BLOCKED", None, 0.05);
        let v = build(
            &p,
            "INCONCLUSIVE",
            0.5,
            0.8,
            "невыполненных проверок на вес 0.25 > 0.2",
            &[blocked],
            &[],
            &[],
            &[],
            0.25,
            0.2,
            &BTreeMap::new(),
        );
        let ids: Vec<&str> = v.groups[0].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["blocked-weight"],
            "cap breach sits in the decision block"
        );
        assert_eq!(v.groups[0].items[0].kind, "cap");
        assert!(v.groups[0].items[0].summary.contains("0.25 > 0.20"));
        // The blocked check itself is «не проверено», never duplicated here.
        assert_eq!(v.groups[3].items.len(), 1);
        assert_eq!(v.groups[3].items[0].id, "brk/x");
    }

    #[test]
    fn the_visible_line_stays_code_free_while_evidence_keeps_the_body() {
        let v = fixture();
        let floor = &v.groups[0].items[1];
        assert!(
            !floor.summary.contains('`') && !floor.summary.contains("fn step"),
            "no code may be inlined into the visible line: {}",
            floor.summary
        );
        assert_eq!(
            floor.site, "src/engine.rs:412",
            "the site is lifted OUT of the paragraph into its own column"
        );
        // The full body is preserved in the collapsed layer, untouched.
        assert!(floor.evidence[0].contains("fn step"));
        let big = &v.groups[1].items[0];
        assert_eq!(big.site, "src/a.rs:10");
        assert!(big.evidence[0].contains("cargo test"));
    }

    #[test]
    fn render_is_a_pure_layout_and_hides_empty_groups() {
        let v = fixture();
        let a = render_text(&v);
        let b = render_text(&v);
        assert_eq!(a, b, "rendering is a pure layout of stored fields");
        assert!(a.contains(TITLE_BLOCKS));
        assert!(a.contains("важность 0.120"));
        assert!(a.contains("закреплено: решение 01a1b1b8-dec (guerrero)"));
        assert!(a.contains("└ доказательства"));
        // An empty group prints no heading (the §2.6 conditional-section
        // precedent) — same form, one group stripped.
        let mut empty = v.clone();
        empty.groups[3].items.clear();
        let text = render_text(&empty);
        assert!(!text.contains(TITLE_UNCHECKED), "{text}");
    }
}
