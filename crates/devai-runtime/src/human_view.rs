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
//! 1. blocks — «бьёт по вердикту / требует твоего решения» (the S<theta
//!    breach itself, gates, floor breaches, cap, mechanical fails, pending
//!    human signature);
//! 2. nuances — ONLY the points still below 1.0 (ratified OQ-QH-1: no
//!    0.8–0.9 band filter; 0.4 without a breach is still the human's
//!    business), ordered by lost-points×weight. A fully passed point (1.0)
//!    WITHOUT a ratified_by link is the NORM (decision of 07.11, R4): it is
//!    not a finding, so it is counted in the headline and appears in no group;
//! 3. anchors — ONLY positions with an EXPLICIT ratified_by link to a stored
//!    decision; names only, junk-drawer forbidden (ratified OQ-QH-2);
//! 4. unchecked — BLOCKED checks with their reason and weight, and any check
//!    state this build does not know (named, never silently folded into a
//!    known branch).
//!
//! Code and probe output live in the per-item `evidence` layer (REQ-QH-04),
//! never inside the visible one-line summary.

pub mod present;

use std::collections::BTreeMap;

// The quality-report contract THIS platform assembles and publishes lives in
// ONE declaration (`devai_core::types`), shared with the store-side prompt
// text; re-exported here so assembly/publication keep reading it from the
// human-form module (rework R3).
pub use devai_core::types::QUALITY_REPORT_CONTRACT;

use devai_core::vocab::{CheckState, ItemKind};
use devai_store::config::Profile;
use serde::{Deserialize, Serialize};

use crate::evaluation::{CheckOutcome, GateOutcome, ProbeState};

/// Version of the presented shape itself (the report contract versions the
/// document; this versions the human form inside it). v2: the verdict-
/// threshold row in group 1, `counts.passed`, and 1.0-without-a-link points
/// are no longer findings (decision of 07.11, R4).
pub const HUMAN_VIEW_VERSION: u32 = 2;

pub const GROUP_BLOCKS: &str = "blocks";
pub const GROUP_NUANCES: &str = "nuances";
pub const GROUP_ANCHORS: &str = "anchors";
pub const GROUP_UNCHECKED: &str = "unchecked";

pub const TITLE_BLOCKS: &str = "БЬЁТ ПО ВЕРДИКТУ / ТРЕБУЕТ ТВОЕГО РЕШЕНИЯ";
pub const TITLE_NUANCES: &str = "НЮАНСЫ";
pub const TITLE_ANCHORS: &str = "ЯКОРЯ / РАТИФИЦИРОВАНО";
pub const TITLE_UNCHECKED: &str = "НЕ ПРОВЕРЕНО / ЖДЁТ";

/// The one-line orientation of a group, stored (not written by the renderer)
/// so the CLI text and the panel read the same sentence.
pub const LEAD_BLOCKS: &str = "их единицы: порог вердикта, нарушенные полы, проваленные шлюзы, блокирующее и места под подпись";
pub const LEAD_NUANCES: &str = "построчно: проверка, балл, вес, важность, сайт, суть; чем больше потерянных баллов на вес, тем выше";
pub const LEAD_ANCHORS: &str =
    "«закреплено» — чьим решением держится; без отсылки на решение пункт сюда не попадает";

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
    /// Blocked weight as decided at assembly, carried in the form as a fact
    /// (the cap row states its own comparison, so no reader derives it, and
    /// the renderer prints no total — it is here for consumers of the shape).
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
    /// The «S / threshold» figures ALREADY formatted by the one formatter
    /// (`fmt_num`) — every reader prints this string, none re-formats the
    /// numbers (rework R3: one precision, one place).
    pub numbers: String,
    /// Counts per group — the «what hits the verdict, what is noise» line.
    pub counts: HumanCounts,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct HumanCounts {
    pub blocks: usize,
    pub nuances: usize,
    pub anchors: usize,
    pub unchecked: usize,
    /// Fully passed points (1.0, no decision link) — the NORM (R4), so they
    /// are counted, not listed. `default` keeps v1 forms readable.
    #[serde(default)]
    pub passed: usize,
}

impl HumanCounts {
    /// Counts read OFF THE FINAL GROUPS — the headline never counts from a
    /// second source that could drift from what is actually presented.
    fn of(groups: &[HumanGroup], passed: usize) -> Self {
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
            passed,
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
    /// The COMPLETE visible row (id, score, weight, importance, site — or the
    /// anchor sentence), formatted by the only formatter at assembly. Every
    /// channel prints this string verbatim, so the CLI and the panel cannot
    /// drift into two wordings (rework R3).
    pub line: String,
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
    let (groups, passed) = collect(
        &ctx,
        checks,
        gates,
        pending,
        blocked_weight,
        max_blocked_weight,
        score,
        theta,
    );
    HumanReportView {
        version: HUMAN_VIEW_VERSION,
        lang: "ru".into(),
        headline: headline_of(machine_verdict, score, theta, basis, &groups, passed),
        groups,
        blocked_weight,
        max_blocked_weight,
    }
}

/// Every row of the form, in one pass: the verdict threshold, the cap, the
/// gates, then one row per check. Classification lives in `Buckets`,
/// ordering in `into_groups`. Returns the groups plus the count of fully
/// passed points (R4: the norm, counted instead of listed).
#[allow(clippy::too_many_arguments)]
fn collect(
    ctx: &Assembly,
    checks: &[CheckOutcome],
    gates: &[GateOutcome],
    pending: &[String],
    blocked_weight: f64,
    max_blocked_weight: f64,
    score: f64,
    theta: f64,
) -> (Vec<HumanGroup>, usize) {
    let mut buckets = Buckets::default();
    buckets.push_threshold(score, theta);
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
    passed: usize,
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
        numbers: format!("S={}/1.00, порог {}", fmt_num(score), fmt_num(theta)),
        counts: HumanCounts::of(groups, passed),
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
        self.breached.contains(&id)
    }

    fn ratified_by(&self, id: &str) -> Option<&'a Ratified> {
        self.ratified.get(id)
    }
}

/// Decision tiers of group 1 («бьёт по вердикту»), in presentation order.
/// Named constants, not bare numbers (rework R10): a row is placed by the
/// weight of its decision, and the order is readable where it is defined.
const TIER_THRESHOLD: u8 = 0;
const TIER_CAP_OR_GATE: u8 = 1;
const TIER_PENDING_SIGNATURE: u8 = 2;
const TIER_FLOOR_BREACH: u8 = 3;
const TIER_MECHANICAL_FAIL: u8 = 4;

/// The four groups under construction, each with its own ordering key.
#[derive(Default)]
struct Buckets {
    /// `(tier, importance, item)`: the tier keeps threshold < cap/gate <
    /// pending < floor < mechanical; inside a tier importance desc, then id.
    blocks: Vec<(u8, Option<f64>, HumanItem)>,
    nuances: Vec<(Option<f64>, HumanItem)>,
    anchors: Vec<HumanItem>,
    unchecked: Vec<(f64, HumanItem)>,
    /// Fully passed points with no decision link (R4): the norm, counted.
    passed: usize,
}

impl Buckets {
    /// The verdict's own threshold (rework R3): «нарушенные пороги» of group 1
    /// includes S below theta, not only per-type floors. `score` and `theta`
    /// are decided-at-assembly values handed to the form — this row states
    /// the fact, it does not re-decide the verdict.
    fn push_threshold(&mut self, score: f64, theta: f64) {
        if score + LOST_SCORE_EPSILON >= theta {
            return;
        }
        self.blocks.push((
            TIER_THRESHOLD,
            None,
            HumanItem {
                id: "score-threshold".into(),
                kind: ItemKind::Threshold.as_str().into(),
                summary: format!(
                    "S={} ниже порога {} — вердикт не может быть пройден",
                    fmt_num(score),
                    fmt_num(theta)
                ),
                line: String::new(),
                site: String::new(),
                score: Some(score),
                weight: None,
                importance: None,
                ratified_by: None,
                reason: None,
                evidence: Vec::new(),
            },
        ));
    }

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
                    "невыполненных проверок на вес {} > {} — вердикт не может быть готовым",
                    fmt_num(blocked_weight),
                    fmt_num(max_blocked_weight)
                ),
                line: String::new(),
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

    /// A gate that is not PASS (failed OR unperformed) is a decision row of
    /// this group, not a nuance. It is NOT printed first: `sort_all` orders
    /// the group by tier, then importance desc, then id — the cap row
    /// (`blocked-weight`) shares the cap/gate tier with the gates (R1: the
    /// old comment claimed an order the sort never applied).
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
                    line: String::new(),
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
                line: String::new(),
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
                line: String::new(),
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
    /// exactly one place: a ratified anchor, a decision row, the nuances, the
    /// passed count (R4: 1.0 with no link is the norm), or «не проверено»
    /// when there is no number to weigh.
    fn push_scored(&mut self, ctx: &Assembly, c: &CheckOutcome) {
        let statement = ctx.statement(&c.id);
        let is_breach = ctx.is_breached(&c.id);
        let is_mech_fail = CheckState::parse(&c.state) == Some(CheckState::MechanicalFail);
        let quiet_pass = is_perfect(c) && !is_breach && !is_mech_fail;
        // OQ-QH-2 (ratified): ONLY an explicit decision link is an anchor, and
        // only a point with nothing to fix — a breached anchored check is a
        // decision item, never filed away as a name.
        if let Some(link) = ctx.ratified_by(&c.id) {
            if quiet_pass {
                self.anchors
                    .push(ratified_item(c, statement, link.display()));
                return;
            }
        } else if quiet_pass {
            // R4 (decision of 07.11): a fully passed point with no decision
            // link is the NORM — counted, never a nuance and never a finding.
            self.passed += 1;
            return;
        }
        let item = scored_item(c, statement, ctx, is_breach, is_mech_fail);
        self.place_scored(c, statement, item, is_breach, is_mech_fail);
    }

    /// Where a scored point that needs a row belongs — ONE rule, decided at
    /// assembly: a floor breach or a mechanical fail is a decision row; every
    /// other scored point still below 1.0 is a nuance; a scored point
    /// without a number is named as unchecked, since there is nothing to weigh.
    fn place_scored(
        &mut self,
        c: &CheckOutcome,
        statement: &str,
        item: HumanItem,
        is_breach: bool,
        is_mech_fail: bool,
    ) {
        if is_breach {
            self.blocks.push((TIER_FLOOR_BREACH, item.importance, item));
        } else if is_mech_fail {
            self.blocks
                .push((TIER_MECHANICAL_FAIL, item.importance, item));
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
                line: String::new(),
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
    fn into_groups(mut self) -> (Vec<HumanGroup>, usize) {
        self.sort_all();
        let blocks = self.blocks.into_iter().map(|(_, _, i)| i);
        let nuances = self.nuances.into_iter().map(|(_, i)| i);
        let unchecked = self.unchecked.into_iter().map(|(_, i)| i);
        (
            vec![
                group(GROUP_BLOCKS, TITLE_BLOCKS, LEAD_BLOCKS, blocks),
                group(GROUP_NUANCES, TITLE_NUANCES, LEAD_NUANCES, nuances),
                group(
                    GROUP_ANCHORS,
                    TITLE_ANCHORS,
                    LEAD_ANCHORS,
                    self.anchors.into_iter(),
                ),
                group(GROUP_UNCHECKED, TITLE_UNCHECKED, "", unchecked),
            ],
            self.passed,
        )
    }

    /// One total order per group (R1: sorting is its own unit, separate from
    /// the mapping into groups).
    fn sort_all(&mut self) {
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
    }
}

/// One group of the stored form. The lead is a fixed sentence of orientation
/// per group; rows are finished (visible `line` frozen) on the way in.
fn group(key: &str, title: &str, lead: &str, items: impl Iterator<Item = HumanItem>) -> HumanGroup {
    HumanGroup {
        key: key.to_string(),
        title: title.to_string(),
        lead: lead.to_string(),
        items: items.map(finish_line).collect(),
    }
}

/// Freeze an item's COMPLETE visible row at assembly (rework R3): the string
/// every channel prints. Decided here, once, so the CLI and the panel cannot
/// print the same point with two wordings or two precisions.
fn finish_line(mut it: HumanItem) -> HumanItem {
    it.line = visible_line(&it);
    it
}

/// A name-only row of the anchors group: kept in force by a NAMED ratified
/// decision (ratified OQ-QH-2 — no link, no anchor).
fn ratified_item(c: &CheckOutcome, statement: &str, ratified_by: String) -> HumanItem {
    HumanItem {
        id: c.id.clone(),
        kind: ItemKind::Anchor.as_str().into(),
        summary: one_line(statement, "закреплено решением"),
        line: String::new(),
        site: String::new(),
        score: c.score,
        weight: Some(c.weight),
        importance: None,
        ratified_by: Some(ratified_by),
        reason: None,
        evidence: Vec::new(),
    }
}

/// The visible row of a scored point that needs one (a floor breach, a
/// mechanical fail, a point below 1.0). A fully passed point without a
/// decision link never reaches here — it is counted, not listed (R4).
/// `importance` is `None` only when the point carries no score.
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
    } else {
        ItemKind::Scored
    };
    HumanItem {
        id: c.id.clone(),
        kind: kind.as_str().into(),
        summary: visible_summary(c, statement),
        site: visible_site(c),
        line: String::new(),
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
        line: String::new(),
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
/// per-group counts — all read off the stored struct. The count of fully
/// passed points is printed when there is one, so the norm is accounted for
/// instead of hidden (R4).
fn render_headline(h: &HumanHeadline) -> Vec<String> {
    let mut counts = format!(
        "бьёт по вердикту: {} · нюансы: {} · якоря: {} · не проверено: {}",
        h.counts.blocks, h.counts.nuances, h.counts.anchors, h.counts.unchecked
    );
    if h.counts.passed > 0 {
        counts.push_str(&format!(" · пройдено без замечаний: {}", h.counts.passed));
    }
    vec![
        "ОТЧЁТ КАЧЕСТВА · человеческая форма".to_string(),
        String::new(),
        format!(
            "ВЕРДИКТ: {} ({}) — {}: {}",
            h.verdict, h.machine_verdict, h.numbers, h.basis
        ),
        counts,
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
            out.push(it.line.clone());
            continue;
        }
        render_item(out, ii, it);
    }
}

/// One stored item as the subordinated block of the CLI channel (REQ-QH-04):
/// the short visible row, the essence, the reason, then the collapsed proof
/// layer indented UNDER it — never code inside the sentence.
fn render_item(out: &mut Vec<String>, ii: usize, it: &HumanItem) {
    out.push(format!("  {}. {}", ii + 1, it.line));
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

/// The COMPLETE visible row of one item — the single formatter of what a
/// human sees (REQ-QH-02, rework R3): an anchor keeps its name-and-ratifier
/// sentence, every other point carries id · score · weight · importance · site.
fn visible_line(it: &HumanItem) -> String {
    if it.kind == ItemKind::Anchor.as_str() {
        anchor_line(it)
    } else {
        item_headline(it)
    }
}

/// One name-only row of the anchors group — the SAME line in every channel.
/// An anchor is only created WITH a ratified link (`ratified_item`, ratified
/// OQ-QH-2), so «закреплено: <decision>» is the row. The `None` arm is a
/// defensive fallback for an item built outside `build` (nothing in the
/// assembly produces an unlinked anchor after the 07.11 decision): it prints
/// the ordinary scored row rather than an empty «закреплено:».
fn anchor_line(it: &HumanItem) -> String {
    match &it.ratified_by {
        Some(by) => format!("  · {} — закреплено: {}", it.id, by),
        None => item_headline(it),
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
        assert_eq!(v.headline.counts.blocks, 3);

        // Blocks: the S<theta fact first (R3), then the unsigned human point
        // and the breached floor, no noise.
        let blocks: Vec<&str> = v.groups[0].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(blocks, vec!["score-threshold", "pend/p", "blk/floor"]);
        assert_eq!(v.groups[0].items[0].kind, "threshold");
        let floor = &v.groups[0].items[2];
        assert_eq!(floor.kind, "floor");
        assert_eq!(floor.score, Some(0.4));
        assert_eq!(floor.weight, Some(0.20));
        // importance = (1-0.4)*0.20
        assert!((floor.importance.unwrap() - 0.12).abs() < 1e-9);

        // Nuances: EVERY remaining point still below 1.0 (0.3 included —
        // ratified OQ-QH-1 has no 0.8-0.9 band), most lost-points×weight
        // first. A fully passed point without a decision link is NOT here
        // (R4, decision of 07.11): it is the norm, counted in the headline.
        let nuances: Vec<&str> = v.groups[1].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(nuances, vec!["n/big", "n/small"]);
        assert!((v.groups[1].items[0].importance.unwrap() - 0.35).abs() < 1e-9);
        assert!((v.groups[1].items[1].importance.unwrap() - 0.01).abs() < 1e-9);
        for it in &v.groups[1].items {
            assert!(it.score.is_some() && it.weight.is_some() && it.importance.is_some());
            assert!(
                it.importance.unwrap() > 0.0,
                "a nuance is a real loss of score"
            );
        }
        assert_eq!(v.headline.counts.passed, 1);

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
        // Every presented point landed somewhere, exactly once; the fully
        // passed `ok/full` is counted, not presented (R4).
        for id in [
            "score-threshold",
            "blk/floor",
            "n/big",
            "n/small",
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
        let passed_hits = v
            .groups
            .iter()
            .flat_map(|g| g.items.iter())
            .filter(|i| i.id == "ok/full")
            .count();
        assert_eq!(passed_hits, 0, "a 1.0 point with no link is not a finding");
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
            if s == CheckState::MechanicalPass {
                // R4 (decision of 07.11): a 1.0 with no decision link is the
                // norm — counted in the headline, never a finding.
                assert!(hits.is_empty(), "a passed point is not presented");
                continue;
            }
            assert_eq!(hits.len(), 1, "{} must be in exactly one group", s.as_str());
            assert_ne!(
                hits[0].kind,
                "unknown",
                "{} is a state the producer emits — never an unknown token",
                s.as_str()
            );
        }
        // The passed mechanical check is accounted for in the counts, not as
        // a hole in the report.
        assert_eq!(v.headline.counts.passed, 1);
    }

    /// R3 (rework): group 1 carries the verdict's own threshold breach — the
    /// S<theta fact is a decision row, not only a sentence in the headline.
    #[test]
    fn a_verdict_threshold_breach_is_a_decision_row_first() {
        let p = profile();
        let v = build(
            &p,
            "FAIL",
            0.55,
            0.8,
            "S below the threshold",
            &[],
            &[],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        assert_eq!(v.groups[0].items.len(), 1);
        let row = &v.groups[0].items[0];
        assert_eq!(row.id, "score-threshold");
        assert_eq!(row.kind, "threshold");
        assert_eq!(row.score, Some(0.55));
        assert!(row.summary.contains("0.550") && row.summary.contains("0.800"));

        // At or above theta there is no threshold row: the fact is absent, so
        // no row is invented.
        let ok = build(
            &p,
            "PASS",
            0.8,
            0.8,
            "at the threshold",
            &[],
            &[],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        assert!(ok.groups[0].items.is_empty());
    }

    /// R4 (decision of 07.11): a fully passed point WITHOUT a ratified_by
    /// link is the norm — it is counted in the headline and must NOT appear
    /// in «нюансы» (which carry only points still below 1.0).
    #[test]
    fn a_fully_passed_point_without_a_link_is_the_norm_not_a_nuance() {
        let p = profile();
        let full = check("ok/full", "SCORED", Some(1.0), 0.20);
        let short = check("n/small", "SCORED", Some(0.9), 0.10);
        let v = build(
            &p,
            "PASS",
            0.95,
            0.8,
            "b",
            &[full, short],
            &[],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        let nuances: Vec<&str> = v.groups[1].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(nuances, vec!["n/small"]);
        assert!(
            !v.groups
                .iter()
                .flat_map(|g| g.items.iter())
                .any(|i| i.id == "ok/full"),
            "1.0 without ratified_by is not presented in any group"
        );
        assert_eq!(v.headline.counts.passed, 1);
        assert!(
            !render_text(&v).contains("ok/full"),
            "the CLI text does not list a norm point"
        );
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
            vec!["score-threshold", "blocked-weight"],
            "cap breach sits in the decision block"
        );
        assert_eq!(v.groups[0].items[1].kind, "cap");
        // One precision for every figure of the form (`fmt_num`), the same
        // formatter the row's own weight/summary uses (rework R3/mt-dry).
        assert!(
            v.groups[0].items[1].summary.contains("0.250 > 0.200"),
            "{}",
            v.groups[0].items[1].summary
        );
        // The blocked check itself is «не проверено», never duplicated here.
        assert_eq!(v.groups[3].items.len(), 1);
        assert_eq!(v.groups[3].items[0].id, "brk/x");
    }

    #[test]
    fn an_unperformed_gate_is_a_decision_row() {
        let p = profile();
        let gate = GateOutcome {
            id: "ci-green".into(),
            state: ProbeState::Blocked,
            detail: "no toolchain in the sandbox".into(),
        };
        let v = build(
            &p,
            "INCONCLUSIVE",
            0.9,
            0.8,
            "b",
            &[],
            &[gate],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        assert_eq!(v.groups[0].items[0].id, "ci-green");
        assert_eq!(v.groups[0].items[0].kind, "gate-unperformed");
        assert_eq!(
            v.groups[0].items[0].reason.as_deref(),
            Some("no toolchain in the sandbox"),
            "the gate detail is carried as the stated reason"
        );
    }

    #[test]
    fn a_breached_anchored_point_stays_a_decision_row() {
        let p = profile();
        let mut anchored = check("anch/a", "SCORED", Some(0.4), 0.20);
        anchored.floor = Some(0.7);
        let v = build(
            &p,
            "FAIL",
            0.5,
            0.8,
            "b",
            &[anchored],
            &[],
            &["anch/a=0.40<floor 0.70".to_string()],
            &[],
            0.0,
            0.2,
            &ratified_map(),
        );
        let blocks: Vec<&str> = v.groups[0].items.iter().map(|i| i.id.as_str()).collect();
        assert!(
            blocks.contains(&"anch/a"),
            "a breached point is a decision row, never filed away as a name: {blocks:?}"
        );
        assert!(v.groups[2].items.is_empty(), "the anchor list stays clean");
        assert_eq!(
            v.groups[0]
                .items
                .iter()
                .find(|i| i.id == "anch/a")
                .and_then(|i| i.ratified_by.as_deref()),
            Some("решение 01a1b1b8-dec (guerrero)"),
            "the link is still shown on the decision row"
        );
    }

    #[test]
    fn a_scored_point_without_a_score_is_named_unchecked() {
        let p = profile();
        let v = build(
            &p,
            "INCONCLUSIVE",
            0.5,
            0.8,
            "b",
            &[check("n/z", "SCORED", None, 0.10)],
            &[],
            &[],
            &[],
            0.0,
            0.2,
            &BTreeMap::new(),
        );
        assert_eq!(v.groups[3].items[0].kind, "no-score");
        assert_eq!(v.groups[3].items[0].weight, Some(0.10));
        assert_eq!(
            v.groups[3].items[0].reason.as_deref(),
            Some("состояние SCORED без балла — предъявить нечего")
        );
    }

    #[test]
    fn a_ratified_link_without_an_actor_names_only_the_decision() {
        let link = Ratified {
            decision_id: "01a1b1b8-dec".into(),
            actor: "  ".into(),
        };
        assert_eq!(link.display(), "решение 01a1b1b8-dec");
        let named = Ratified {
            decision_id: "01a1b1b8-dec".into(),
            actor: "guerrero".into(),
        };
        assert_eq!(named.display(), "решение 01a1b1b8-dec (guerrero)");
    }

    #[test]
    fn the_visible_line_stays_code_free_while_evidence_keeps_the_body() {
        let v = fixture();
        let floor = &v.groups[0].items[2];
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
            a.contains("score-threshold"),
            "the S<theta breach is a row of group 1: {a}"
        );
        assert!(
            a.contains("пройдено без замечаний: 1"),
            "the norm point is accounted for in the counts: {a}"
        );
        assert!(
            !a.contains("ok/full"),
            "a fully passed point with no link is not listed: {a}"
        );
        assert!(a.contains("└ доказательства"));
        // An empty group prints no heading (the §2.6 conditional-section
        // precedent) — same form, one group stripped.
        let mut empty = v.clone();
        empty.groups[3].items.clear();
        let text = render_text(&empty);
        assert!(!text.contains(TITLE_UNCHECKED), "{text}");
    }

    /// R6: the rendered human text of ONE fixture, byte for byte (the fixture
    /// lives in `tests/fixtures/human_view_cli_text.txt`). Every wording and
    /// precision change of the CLI form shows up in the test diff instead of
    /// shipping silently.
    #[test]
    fn rendered_cli_text_is_pinned_byte_for_byte() {
        const PINNED: &str = include_str!("../tests/fixtures/human_view_cli_text.txt");
        assert_eq!(render_text(&fixture()), PINNED.trim_end_matches('\n'));
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
        assert_eq!(blocks, vec!["score-threshold", "pend/p", "blk/floor"]);
    }

    /// The CURRENT quality-report contract, as seeded (highest version).
    fn quality_report_schema() -> serde_json::Value {
        let (_, _, raw) = devai_store::seed::contracts()
            .into_iter()
            .filter(|(name, _, _)| *name == "quality-report")
            .max_by_key(|(_, version, _)| *version)
            .expect("the quality-report contract is seeded");
        serde_json::from_str(&raw).expect("the seeded schema is valid JSON")
    }

    /// R1 (da/api-contract 0.7): the seeded document schema and the reader are
    /// ONE shape, checked over the form THIS writer emits:
    /// every key the schema marks `required` is emitted; every emitted key is
    /// declared; and `required` is exactly the set the reader cannot load
    /// without — a declared-only key removed still loads, a required one does
    /// not. A field the reader needs but the schema leaves optional (the bug
    /// this test exists for: `line`, `headline.numbers`) fails here instead of
    /// surfacing as a «повреждённая форма» in production.
    #[test]
    fn the_seeded_schema_and_the_reader_are_the_same_shape() {
        let schema = quality_report_schema();
        let value = serde_json::to_value(fixture()).expect("the form serialises");
        let mut steps = Vec::new();
        let mut checks = Vec::new();
        assert_shape(
            &schema["properties"]["human_view"],
            &value,
            "human_view",
            &mut steps,
            &mut checks,
        );
        for (steps, required) in checks {
            let path = steps_path(&steps);
            let mut missing = value.clone();
            remove_at(&mut missing, &steps);
            let loads = serde_json::from_value::<HumanReportView>(missing);
            if required {
                assert!(
                    loads.is_err(),
                    "{path} is required by the schema, yet the reader loads a form without it"
                );
            } else {
                assert!(
                    loads.is_ok(),
                    "{path} is optional in the schema, yet the reader cannot load without it: {:?}",
                    loads.err()
                );
            }
        }
    }

    /// One step of a walk over the emitted JSON: an object key or an array
    /// element, so a failed check can name (and remove) the exact field.
    #[derive(Clone, Debug)]
    enum Step {
        Key(String),
        Index(usize),
    }

    fn steps_path(steps: &[Step]) -> String {
        let mut out = String::from("human_view");
        for s in steps {
            match s {
                Step::Key(k) => out.push_str(&format!(".{k}")),
                Step::Index(i) => out.push_str(&format!("[{i}]")),
            }
        }
        out
    }

    fn remove_at(root: &mut serde_json::Value, steps: &[Step]) {
        let (last, path) = steps.split_last().expect("a check always has a step");
        let mut cur = root;
        for s in path {
            cur = match s {
                Step::Key(k) => &mut cur[k],
                Step::Index(i) => &mut cur[*i],
            };
        }
        match last {
            Step::Key(k) => {
                cur.as_object_mut().expect("keyed step").remove(k);
            }
            Step::Index(i) => {
                cur.as_array_mut().expect("indexed step").remove(*i);
            }
        }
    }

    /// Walk the writer's own output against the schema. Collects (for every
    /// emitted key) its path and whether the schema marks it required, so the
    /// caller can prove the split both ways on the whole document.
    fn assert_shape(
        schema: &serde_json::Value,
        value: &serde_json::Value,
        path: &str,
        steps: &mut Vec<Step>,
        checks: &mut Vec<(Vec<Step>, bool)>,
    ) {
        if let Some(arr) = value.as_array() {
            let items = schema
                .get("items")
                .unwrap_or_else(|| panic!("{path}: the schema declares no array items"));
            for (i, v) in arr.iter().enumerate() {
                steps.push(Step::Index(i));
                assert_shape(items, v, &format!("{path}[{i}]"), steps, checks);
                steps.pop();
            }
            return;
        }
        let Some(obj) = value.as_object() else {
            return;
        };
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .unwrap_or_else(|| panic!("{path}: the schema declares no properties"));
        let required: Vec<&str> = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        for name in &required {
            assert!(
                obj.contains_key(*name),
                "schema requires {path}.{name}, the writer does not emit it"
            );
        }
        for (key, child) in obj {
            let child_schema = props.get(key).unwrap_or_else(|| {
                panic!("the writer emits {path}.{key}, the schema does not declare it")
            });
            steps.push(Step::Key(key.clone()));
            assert_shape(child_schema, child, &format!("{path}.{key}"), steps, checks);
            checks.push((steps.clone(), required.contains(&key.as_str())));
            steps.pop();
        }
    }
}
