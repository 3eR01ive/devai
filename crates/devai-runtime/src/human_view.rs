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
//!   2. nuances   — every remaining scored point (ratified OQ-QH-1: no
//!                  0.8–0.9 band filter; 0.4 without a breach is still the
//!                  human's business), ordered by lost-points×weight. A point
//!                  scored 1.0 WITHOUT a ratified_by link has no other group
//!                  to go to (OQ-QH-2: not an anchor), so it lands here with
//!                  importance 0 — presented, never silently dropped;
//!   3. anchors   — ONLY positions with an EXPLICIT ratified_by link to a
//!                  stored decision; names only, junk-drawer forbidden
//!                  (ratified OQ-QH-2);
//!   4. unchecked — BLOCKED checks with their reason and weight, and any
//!                  check state this build does not know (named, never
//!                  silently folded into a known branch).
//!
//! Code and probe output live in the per-item `evidence` layer (REQ-QH-04),
//! never inside the visible one-line summary.

pub mod present;

use std::collections::BTreeMap;

use devai_core::vocab::{CheckState, ItemKind};
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

impl HumanCounts {
    /// Counts read OFF THE FINAL GROUPS — the headline never counts from a
    /// second source that could drift from what is actually presented.
    fn of(groups: &[HumanGroup]) -> Self {
        let len = |key: &str| {
            groups
                .iter()
                .find(|g| g.key == key)
                .map_or(0, |g| g.items.len())
        };
        Self {
            blocks: len(GROUP_BLOCKS),
            nuances: len(GROUP_NUANCES),
            anchors: len(GROUP_ANCHORS),
            unchecked: len(GROUP_UNCHECKED),
        }
    }
}

/// The check states live in ONE place (`devai_core::vocab::CheckState`) — the
/// producer, this parser and the stored schema all read the same constants
/// (rework R3/R5). An unknown token is `None`, never a default.

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
    /// mechanical | scored | blocked | cap | anchor | passed | no-score |
    /// unknown.
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
    let ctx = Assembly::new(profile, floors_breached, ratified);
    let groups = collect(
        &ctx,
        checks,
        gates,
        pending,
        blocked_weight,
        max_blocked_weight,
    );
    HumanReportView {
        version: HUMAN_VIEW_VERSION,
        lang: "ru".into(),
        headline: headline_of(machine_verdict, score, theta, basis, &groups),
        groups,
        blocked_weight,
        max_blocked_weight,
    }
}

/// Every row of the form, in one pass: the cap row, the gates, then one row
/// per check. Classification lives in `Buckets`, ordering in `into_groups`.
fn collect(
    ctx: &Assembly,
    checks: &[CheckOutcome],
    gates: &[GateOutcome],
    pending: &[String],
    blocked_weight: f64,
    max_blocked_weight: f64,
) -> Vec<HumanGroup> {
    let mut buckets = Buckets::default();
    buckets.push_cap(blocked_weight, max_blocked_weight);
    buckets.push_gates(gates);
    for c in checks {
        buckets.push_check(ctx, c, pending);
    }
    buckets.into_groups()
}

/// The verdict line and the «what hits the verdict, what is noise» counts —
/// read OFF THE FINAL GROUPS, never from a second source that could drift.
fn headline_of(
    machine_verdict: &str,
    score: f64,
    theta: f64,
    basis: &str,
    groups: &[HumanGroup],
) -> HumanHeadline {
    HumanHeadline {
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
        counts: HumanCounts::of(groups),
    }
}

/// Float-noise floor for «this point actually lost score». Rubric scores have
/// 0.01 granularity and the weights sum to 1, so a real loss is many orders of
/// magnitude above this; anything at or below it is the f64 noise of an exact
/// 1.0 and must not turn a passed point into a nuance.
const LOST_SCORE_EPSILON: f64 = 1e-9;

/// ONE number format for every presented figure — score, weight and
/// importance share it, in the CLI text and in the UI, so the same number is
/// never printed with two precisions a human has to reconcile.
fn fmt_num(v: f64) -> String {
    format!("{v:.3}")
}

/// Decided-at-assembly context shared by every classification step: the
/// profile statements (for the visible one-liner) plus the two already-decided
/// flag lists. Read-only — nothing here is derived a second time.
struct Assembly<'a> {
    statements: BTreeMap<&'a str, &'a str>,
    /// Ids parsed from the decided-at-assembly strings ("id=0.40<floor 0.70");
    /// the flag itself is never re-applied to a score here.
    breached: Vec<&'a str>,
    ratified: &'a BTreeMap<String, Ratified>,
}

impl<'a> Assembly<'a> {
    fn new(
        profile: &'a Profile,
        floors_breached: &'a [String],
        ratified: &'a BTreeMap<String, Ratified>,
    ) -> Self {
        Self {
            statements: profile
                .checks()
                .into_iter()
                .map(|c| (c.id.as_str(), c.statement.as_str()))
                .collect(),
            breached: floors_breached
                .iter()
                .filter_map(|s| s.split('=').next())
                .collect(),
            ratified,
        }
    }

    fn statement(&self, id: &str) -> &'a str {
        self.statements.get(id).copied().unwrap_or("")
    }

    fn is_breached(&self, id: &str) -> bool {
        self.breached.iter().any(|b| *b == id)
    }

    fn ratified_by(&self, id: &str) -> Option<&'a Ratified> {
        self.ratified.get(id)
    }
}

/// Decision tiers of group 1 («бьёт по вердикту»), in presentation order.
/// Named constants, not bare numbers (rework R10): a row is placed by the
/// weight of its decision, and the order is readable where it is defined.
const TIER_CAP_OR_GATE: u8 = 0;
const TIER_PENDING_SIGNATURE: u8 = 1;
const TIER_FLOOR_BREACH: u8 = 2;
const TIER_MECHANICAL_FAIL: u8 = 3;

/// The four groups under construction, each with its own ordering key.
#[derive(Default)]
struct Buckets {
    /// `(tier, importance, item)`: the tier keeps cap/gate < pending < floor <
    /// mechanical; inside a tier importance desc, then id.
    blocks: Vec<(u8, Option<f64>, HumanItem)>,
    nuances: Vec<(Option<f64>, HumanItem)>,
    anchors: Vec<HumanItem>,
    unchecked: Vec<(f64, HumanItem)>,
}

impl Buckets {
    /// «Блокирующее сверх кэпа» (REQ-QH-03): when the weight of unperformed
    /// checks alone can hold the verdict hostage, that is a decision item, not
    /// a footnote — the headline basis says it too, the group shows a row.
    fn push_cap(&mut self, blocked_weight: f64, max_blocked_weight: f64) {
        if blocked_weight <= max_blocked_weight {
            return;
        }
        self.blocks.push((
            TIER_CAP_OR_GATE,
            None,
            HumanItem {
                id: "blocked-weight".into(),
                kind: ItemKind::Cap.as_str().into(),
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

    /// Gates first: a failed gate is the decision, not a nuance.
    fn push_gates(&mut self, gates: &[GateOutcome]) {
        for g in gates.iter().filter(|g| g.state != ProbeState::Pass) {
            self.blocks.push((
                TIER_CAP_OR_GATE,
                None,
                HumanItem {
                    id: g.id.clone(),
                    kind: if g.state == ProbeState::Fail {
                        ItemKind::Gate.as_str().into()
                    } else {
                        ItemKind::GateUnperformed.as_str().into()
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
    }

    /// One row for one check — the state decides the group, and every known
    /// state has its own branch (no silent `_` that folds a new state into
    /// «scored»). `None` from the parser is the explicit unknown branch.
    fn push_check(&mut self, ctx: &Assembly, c: &CheckOutcome, pending: &[String]) {
        match CheckState::parse(&c.state) {
            Some(CheckState::PendingHuman) => self.push_pending(ctx, c, pending),
            Some(CheckState::Blocked) => self.push_blocked(ctx, c),
            // MECHANICAL_PASS and SCORED are both scored points: the platform's
            // own 1.0 and the auditor's rubric score go through ONE placement
            // rule (rework R5/R6 — a passed mechanical check used to fall into
            // «не проверено» as an unknown token).
            Some(CheckState::Scored)
            | Some(CheckState::MechanicalPass)
            | Some(CheckState::MechanicalFail) => self.push_scored(ctx, c),
            None => self.push_unknown(ctx, c),
        }
    }

    fn push_pending(&mut self, ctx: &Assembly, c: &CheckOutcome, pending: &[String]) {
        let t = ctx.statement(&c.id);
        self.blocks.push((
            TIER_PENDING_SIGNATURE,
            None,
            HumanItem {
                id: c.id.clone(),
                kind: ItemKind::Pending.as_str().into(),
                summary: one_line(t, "нужна твоя подпись — вердикт не готов"),
                site: String::new(),
                score: None,
                weight: Some(c.weight),
                importance: None,
                ratified_by: None,
                reason: pending
                    .iter()
                    .find(|p| **p == c.id)
                    .map(|p| format!("{p}: подпись человека, черновик балла не финализируется")),
                evidence: c.evidence.clone(),
            },
        ));
    }

    fn push_blocked(&mut self, ctx: &Assembly, c: &CheckOutcome) {
        let t = ctx.statement(&c.id);
        self.unchecked.push((
            c.weight,
            HumanItem {
                id: c.id.clone(),
                kind: ItemKind::Blocked.as_str().into(),
                summary: one_line(t, "проверка не выполнена — балла нет"),
                site: String::new(),
                score: None,
                weight: Some(c.weight),
                importance: None,
                ratified_by: ctx.ratified_by(&c.id).map(|r| r.display()),
                reason: Some(c.reason.clone()).filter(|r| !r.trim().is_empty()),
                evidence: c.evidence.clone(),
            },
        ));
    }

    /// A scored / mechanically passed / mechanically failed check lands in
    /// exactly one place: a ratified anchor, a decision row, the nuances, or
    /// «не проверено» when there is no number to weigh.
    fn push_scored(&mut self, ctx: &Assembly, c: &CheckOutcome) {
        let statement = ctx.statement(&c.id);
        let is_breach = ctx.is_breached(&c.id);
        let is_mech_fail = CheckState::parse(&c.state) == Some(CheckState::MechanicalFail);
        // OQ-QH-2 (ratified): ONLY an explicit decision link is an anchor, and
        // only a point with nothing to fix — a breached anchored check is a
        // decision item, never filed away as a name.
        if let Some(link) = ctx.ratified_by(&c.id) {
            if is_perfect(c) && !is_breach && !is_mech_fail {
                self.anchors
                    .push(ratified_item(c, statement, link.display()));
                return;
            }
        }
        let item = scored_item(c, statement, ctx, is_breach, is_mech_fail);
        self.place_scored(c, statement, item, is_breach, is_mech_fail);
    }

    /// Where a non-anchor scored point belongs — ONE rule, decided at
    /// assembly: a floor breach or a mechanical fail is a decision row; every
    /// other scored point with a number is a nuance (fully passed ones too,
    /// at importance 0 — OQ-QH-2 leaves them no other group); a scored point
    /// without a number is named as unchecked, since there is nothing to weigh.
    fn place_scored(
        &mut self,
        c: &CheckOutcome,
        statement: &str,
        item: HumanItem,
        is_breach: bool,
        is_mech_fail: bool,
    ) {
        if is_breach || is_mech_fail {
            let tier = if is_breach {
                TIER_FLOOR_BREACH
            } else {
                TIER_MECHANICAL_FAIL
            };
            self.blocks.push((tier, item.importance, item));
        } else if item.importance.is_some() {
            self.nuances.push((item.importance, item));
        } else {
            self.unchecked.push((c.weight, no_score_item(c, statement)));
        }
    }

    /// A state this build does not recognise: the check is NOT dropped and NOT
    /// guessed into another group — it is shown as unchecked with the raw
    /// token, so a producer/consumer mismatch stays visible.
    fn push_unknown(&mut self, ctx: &Assembly, c: &CheckOutcome) {
        let t = ctx.statement(&c.id);
        self.unchecked.push((
            c.weight,
            HumanItem {
                id: c.id.clone(),
                kind: ItemKind::Unknown.as_str().into(),
                summary: one_line(t, "состояние проверки не распознано"),
                site: String::new(),
                score: c.score,
                weight: Some(c.weight),
                importance: None,
                ratified_by: None,
                reason: Some(format!(
                    "неизвестное состояние проверки «{}» — пункт не предъявлен как решённый",
                    c.state
                )),
                evidence: collapsed_layer(c),
            },
        ));
    }

    /// Freeze the buckets into the four fixed groups, each ordered by decision
    /// weight. Total orders with an id tiebreak: byte-stable.
    fn into_groups(mut self) -> Vec<HumanGroup> {
        self.blocks.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| desc(a.1, b.1))
                .then_with(|| a.2.id.cmp(&b.2.id))
        });
        self.nuances
            .sort_by(|a, b| desc(a.0, b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        self.unchecked
            .sort_by(|a, b| desc(Some(a.0), Some(b.0)).then_with(|| a.1.id.cmp(&b.1.id)));
        self.anchors.sort_by(|a, b| a.id.cmp(&b.id));
        vec![
            HumanGroup {
                key: GROUP_BLOCKS.into(),
                title: TITLE_BLOCKS.into(),
                lead: "их единицы: нарушенные пороги, проваленные шлюзы, блокирующее и места под подпись".into(),
                items: self.blocks.into_iter().map(|(_, _, i)| i).collect(),
            },
            HumanGroup {
                key: GROUP_NUANCES.into(),
                title: TITLE_NUANCES.into(),
                lead: "построчно: проверка, балл, вес, важность, сайт, суть; полностью пройденное — в конце (важность 0)".into(),
                items: self.nuances.into_iter().map(|(_, i)| i).collect(),
            },
            HumanGroup {
                key: GROUP_ANCHORS.into(),
                title: TITLE_ANCHORS.into(),
                lead: "«закреплено» — чьим решением держится; без отсылки на решение пункт сюда не попадает".into(),
                items: self.anchors,
            },
            HumanGroup {
                key: GROUP_UNCHECKED.into(),
                title: TITLE_UNCHECKED.into(),
                lead: String::new(),
                items: self.unchecked.into_iter().map(|(_, i)| i).collect(),
            },
        ]
    }
}

/// A name-only row of the anchors group: kept in force by a NAMED ratified
/// decision (ratified OQ-QH-2 — no link, no anchor).
fn ratified_item(c: &CheckOutcome, statement: &str, ratified_by: String) -> HumanItem {
    HumanItem {
        id: c.id.clone(),
        kind: ItemKind::Anchor.as_str().into(),
        summary: one_line(statement, "закреплено решением"),
        site: String::new(),
        score: c.score,
        weight: Some(c.weight),
        importance: None,
        ratified_by: Some(ratified_by),
        reason: None,
        evidence: Vec::new(),
    }
}

/// The visible row of a scored point that is NOT an anchor. `importance` is
/// `None` only when the point carries no score at all; a point scored 1.0
/// carries `Some(0.0)` so it is presented (importance 0) instead of vanishing.
fn scored_item(
    c: &CheckOutcome,
    statement: &str,
    ctx: &Assembly,
    is_breach: bool,
    is_mech_fail: bool,
) -> HumanItem {
    let kind = if is_breach {
        ItemKind::Floor
    } else if is_mech_fail {
        ItemKind::Mechanical
    } else if is_perfect(c) {
        ItemKind::Passed
    } else {
        ItemKind::Scored
    };
    HumanItem {
        id: c.id.clone(),
        kind: kind.as_str().into(),
        summary: if is_perfect(c) && !is_breach && !is_mech_fail {
            one_line(statement, "пройдено без замечаний")
        } else {
            visible_summary(c, statement)
        },
        site: visible_site(c),
        score: c.score,
        weight: Some(c.weight),
        importance: importance_of(c),
        ratified_by: ctx.ratified_by(&c.id).map(|r| r.display()),
        reason: None,
        evidence: collapsed_layer(c),
    }
}

/// A scored point with no number to weigh: named as unchecked, never as a
/// nuance with an invented score.
fn no_score_item(c: &CheckOutcome, statement: &str) -> HumanItem {
    HumanItem {
        id: c.id.clone(),
        kind: ItemKind::NoScore.as_str().into(),
        summary: one_line(statement, "проверка завершена без балла"),
        site: String::new(),
        score: None,
        weight: Some(c.weight),
        importance: None,
        ratified_by: None,
        reason: Some("состояние SCORED без балла — предъявить нечего".into()),
        evidence: collapsed_layer(c),
    }
}

/// Lost points × weight, decided at assembly (REQ-QH-02). `None` only when
/// there is no score; a 1.0 point yields `Some(0.0)` — a real number, printed
/// as 0.000, so «nothing lost» is visible instead of absent.
fn importance_of(c: &CheckOutcome) -> Option<f64> {
    c.score.map(|s| (1.0 - s).max(0.0) * c.weight).map(|lost| {
        if lost <= LOST_SCORE_EPSILON {
            0.0
        } else {
            lost
        }
    })
}

/// Exactly 1.0 (within f64 noise) — the only score an anchor may carry.
fn is_perfect(c: &CheckOutcome) -> bool {
    c.score.is_some_and(|s| (1.0 - s) <= LOST_SCORE_EPSILON)
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
    let mut out = render_headline(&view.headline);
    for (gi, g) in view.groups.iter().enumerate() {
        render_group(&mut out, gi, g);
    }
    out.join("\n")
}

/// The verdict block: machine verdict, S against the threshold, basis, and the
/// per-group counts — all read off the stored struct.
fn render_headline(h: &HumanHeadline) -> Vec<String> {
    vec![
        "ОТЧЁТ КАЧЕСТВА · человеческая форма".to_string(),
        String::new(),
        format!(
            "ВЕРДИКТ: {} ({}) — S={:.3}/1.00, порог {:.2}: {}",
            h.verdict, h.machine_verdict, h.score, h.theta, h.basis
        ),
        format!(
            "бьёт по вердикту: {} · нюансы: {} · якоря: {} · не проверено: {}",
            h.counts.blocks, h.counts.nuances, h.counts.anchors, h.counts.unchecked
        ),
    ]
}

/// One group: heading, lead, then its rows. An empty group prints no heading
/// (the conditional-section precedent of §2.6 `render`).
fn render_group(out: &mut Vec<String>, gi: usize, g: &HumanGroup) {
    if g.items.is_empty() {
        return;
    }
    out.push(String::new());
    out.push(format!("{}) {} ({})", gi + 1, g.title, g.items.len()));
    if !g.lead.is_empty() {
        out.push(format!("   {}", g.lead));
    }
    for (ii, it) in g.items.iter().enumerate() {
        if g.key == GROUP_ANCHORS {
            out.push(anchor_line(it));
            continue;
        }
        render_item(out, ii, it);
    }
}

/// One stored item as the subordinated block of the CLI channel (REQ-QH-04):
/// the short visible row, the essence, the reason, then the collapsed proof
/// layer indented UNDER it — never code inside the sentence.
fn render_item(out: &mut Vec<String>, ii: usize, it: &HumanItem) {
    out.push(format!("  {}. {}", ii + 1, item_headline(it)));
    if !it.summary.is_empty() {
        out.push(format!("     суть: {}", it.summary));
    }
    if let Some(reason) = &it.reason {
        out.push(format!("     причина: {}", reason));
    }
    if it.evidence.is_empty() {
        return;
    }
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

/// One name-only row of the anchors group — the SAME line in every channel:
/// a ratified link is printed with who ratified it, a fully passed point says
/// so in plain words instead of showing an empty «закреплено:».
fn anchor_line(it: &HumanItem) -> String {
    match &it.ratified_by {
        Some(by) => format!("  · {} — закреплено: {}", it.id, by),
        None => format!(
            "  · {} — полностью пройдено, замечаний нет (балл {})",
            it.id,
            it.score.map(fmt_num).unwrap_or_else(|| "—".into())
        ),
    }
}

/// The single formatter of an item's visible line (REQ-QH-02). The same
/// function produces the CLI line and, through the stored fields, the UI line:
/// score, weight and importance are printed by `fmt_num` and never by a second
/// formatter with its own precision.
fn item_headline(it: &HumanItem) -> String {
    let mut parts: Vec<String> = vec![it.id.clone()];
    match it.score {
        Some(s) => parts.push(format!("балл {}", fmt_num(s))),
        None => parts.push("балл —".into()),
    }
    if let Some(w) = it.weight {
        parts.push(format!("вес {}", fmt_num(w)));
    }
    if let Some(i) = it.importance {
        parts.push(format!("важность {}", fmt_num(i)));
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

        // Nuances: EVERY remaining scored point (0.3 included — ratified
        // OQ-QH-1 has no 0.8-0.9 band), most lost-points×weight first; a
        // fully passed point without a decision link is here too, at
        // importance 0 (OQ-QH-2 gives it no anchor seat).
        let nuances: Vec<&str> = v.groups[1].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(nuances, vec!["n/big", "n/small", "ok/full"]);
        assert!((v.groups[1].items[0].importance.unwrap() - 0.35).abs() < 1e-9);
        assert!((v.groups[1].items[1].importance.unwrap() - 0.01).abs() < 1e-9);
        for it in &v.groups[1].items {
            assert!(it.score.is_some() && it.weight.is_some() && it.importance.is_some());
        }
        assert_eq!(v.groups[1].items[2].kind, "passed");
        assert_eq!(v.groups[1].items[2].importance, Some(0.0));
        assert!(v.groups[1].items[2].ratified_by.is_none());

        // Anchors: names only, and ONLY an explicit ratified_by link — a
        // fully passed point without one is not an anchor (ratified OQ-QH-2).
        assert_eq!(v.headline.counts.anchors, 1);
        let anchors: Vec<&str> = v.groups[2].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(anchors, vec!["anch/a"]);
        assert_eq!(
            v.groups[2].items[0].ratified_by.as_deref(),
            Some("решение 01a1b1b8-dec (guerrero)")
        );
        assert_eq!(v.groups[2].items[0].kind, "anchor");
        assert!(
            v.groups[2].items[0].evidence.is_empty(),
            "anchor = name only"
        );
        assert!(
            !v.groups[2].lead.is_empty(),
            "the lead names what «закреплено» means"
        );

        // Unchecked: the BLOCKED check with its reason and weight.
        assert_eq!(v.groups[3].items.len(), 1);
        assert_eq!(v.groups[3].items[0].id, "brk/x");
        assert_eq!(v.groups[3].items[0].weight, Some(0.05));
        assert_eq!(
            v.groups[3].items[0].reason.as_deref(),
            Some("probe could not run")
        );
        // Every scored/blocked/pending point landed somewhere, exactly once.
        for id in [
            "blk/floor",
            "n/big",
            "n/small",
            "ok/full",
            "anch/a",
            "brk/x",
            "pend/p",
        ] {
            let hits = v
                .groups
                .iter()
                .flat_map(|g| g.items.iter())
                .filter(|i| i.id == id)
                .count();
            assert_eq!(hits, 1, "{id} must be in exactly one group");
        }
    }

    #[test]
    fn an_unknown_check_state_is_named_not_silently_scored() {
        let p = profile();
        let odd = check("brk/x", "SOMETHING_NEW", Some(0.1), 0.05);
        let v = build(
            &p,
            "INCONCLUSIVE",
            0.5,
            0.8,
            "b",
            &[odd],
            &[],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        // The unknown state is presented in «не проверено», with the raw token
        // in the reason — never folded into a group by a silent default.
        assert_eq!(v.groups[3].items.len(), 1);
        assert_eq!(v.groups[3].items[0].kind, "unknown");
        assert!(
            v.groups[3].items[0]
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("SOMETHING_NEW")
        );
        assert!(v.groups[1].items.is_empty(), "not guessed into nuances");
    }

    /// R5/R6a: EVERY state the producer emits is placed by the form — a
    /// passed mechanical check (MECHANICAL_PASS, seven of them in the live
    /// profile) used to fall into «не проверено» as an unknown token. The
    /// parser's total coverage is asserted against the producer vocabulary
    /// itself, so adding a state without teaching the form fails here.
    #[test]
    fn every_emitted_state_lands_in_exactly_one_group() {
        let p = profile();
        let checks: Vec<CheckOutcome> = CheckState::ALL
            .iter()
            .map(|s| {
                let (score, weight) = match s {
                    CheckState::Scored => (Some(0.9), 0.10),
                    CheckState::MechanicalPass => (Some(1.0), 0.00),
                    CheckState::MechanicalFail => (Some(0.0), 0.05),
                    CheckState::Blocked | CheckState::PendingHuman => (None, 0.05),
                };
                let mut c = check(&format!("st/{s:?}"), s.as_str(), score, weight);
                if *s == CheckState::Blocked {
                    c.reason = "probe could not run".into();
                }
                c
            })
            .collect();
        let v = build(
            &p,
            "INCONCLUSIVE",
            0.5,
            0.8,
            "b",
            &checks,
            &[],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        for s in CheckState::ALL {
            let id = format!("st/{s:?}");
            let hits: Vec<&HumanItem> = v
                .groups
                .iter()
                .flat_map(|g| g.items.iter())
                .filter(|i| i.id == id)
                .collect();
            assert_eq!(hits.len(), 1, "{} must be in exactly one group", s.as_str());
            assert_ne!(
                hits[0].kind,
                "unknown",
                "{} is a state the producer emits — never an unknown token",
                s.as_str()
            );
        }
        // The passed mechanical check is a presented point with importance 0,
        // not a hole in the report.
        let passed = v
            .groups
            .iter()
            .flat_map(|g| g.items.iter())
            .find(|i| i.id == "st/MechanicalPass")
            .expect("mechanical pass is presented");
        assert_eq!(passed.kind, "passed");
        assert_eq!(passed.importance, Some(0.0));
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
        assert!(
            a.contains("суть: nothing to fix here"),
            "a fully passed point is presented in the nuances with importance 0, not as an anchor: {a}"
        );
        assert!(
            a.contains("важность 0.000"),
            "its 0.000 importance is printed, so «nothing lost» is visible: {a}"
        );
        assert!(a.contains("└ доказательства"));
        // An empty group prints no heading (the §2.6 conditional-section
        // precedent) — same form, one group stripped.
        let mut empty = v.clone();
        empty.groups[3].items.clear();
        let text = render_text(&empty);
        assert!(!text.contains(TITLE_UNCHECKED), "{text}");
    }

    /// REQ-QH-01 AC3 (rework R5): the same assembly inputs yield a BIT-EQUAL
    /// form on every build. Group membership, ordering and importance come
    /// from `BTreeMap`s and explicit sorts, so this must hold without a
    /// dedupe pass at read time — the test pins it, so a future `HashMap` or
    /// an unstable sort fails here instead of drifting the stored document.
    #[test]
    fn two_builds_of_the_same_inputs_are_bit_equal() {
        let a = serde_json::to_vec(&fixture()).expect("the form serialises");
        let b = serde_json::to_vec(&fixture()).expect("the form serialises");
        assert_eq!(a, b, "assembly must be deterministic for the same inputs");
        // And the deterministic order is the DECISION order, not id order.
        let v: HumanReportView = serde_json::from_slice(&a).expect("round-trips");
        let blocks: Vec<&str> = v.groups[0].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(blocks, vec!["pend/p", "blk/floor"]);
    }
}
