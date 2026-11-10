//! The FIRST LAYER of a stored quality report (REQ-SAY-01..06): what a human
//! reads before expanding anything — «что нашёл аудит», the findings in plain
//! words, and the «не проверено» section kept apart from them.
//!
//! Presentation only, on top of facts the stored form ALREADY decided: the
//! machine verdict token, the group counts, the group membership, and the
//! stored `summary` / `site` / `reason` / `evidence` of every item. Nothing is
//! re-scored, re-grouped or re-ordered. ONE figure does stand in this layer by
//! the operator's decision of 10.11 (REQ-SAY-02): the saved overall score of
//! the work, next to the verbal assessment — read as stored, formatted by the
//! one formatter, never re-computed. Every other number keeps its one home in
//! the stored row (`HumanItem::line`) and is read in the «подробности» layer.
//! An item id is carried along, but only for the collapsed detail of its row.
//!
//! ONE wording for both channels (ratified decision of 10.11: the terminal
//! report and the window change together): `render_text` lays the CLI report
//! out from this view, and the API hands the very same struct to the UI panel.
//! Two surfaces therefore cannot drift into two dictionaries.
//!
//! The rule is domain-free: the sentences speak of «работа», «замечание»,
//! «проверка» — never of a technology, a check id or a project.

use serde::{Deserialize, Serialize};

use devai_core::vocab::ItemKind;

use super::{
    GROUP_BLOCKS, GROUP_NUANCES, GROUP_UNCHECKED, HumanGroup, HumanHeadline, HumanItem,
    HumanReportView,
};

// ---------------------------------------------------------------------------
// Wording. Every sentence of the first layer lives here and nowhere else.
// ---------------------------------------------------------------------------

/// A passed report with no remaining point below 1.0.
pub const FOUND_PASS: &str = "Аудит замечаний не нашёл — работа выглядит готовой.";
/// A passed report whose only points below 1.0 are the ones that do not block.
pub const FOUND_PASS_NUANCES: &str = "В целом годно: принять можно, но есть замечания на будущее.";
/// A report the platform did not pass.
pub const FOUND_FAIL: &str = "Принять как есть нельзя: есть замечания, которые мешают.";
/// No verdict yet.
pub const FOUND_NOT_READY: &str = "Пока не готово: итог ещё не сложился.";
/// The same, when checks were not performed at all.
pub const FOUND_NOT_READY_UNCHECKED: &str =
    " Часть проверок не выполнена — они собраны отдельным разделом, ниже.";
/// A verdict token this build does not read as one of the known outcomes: the
/// report is not rounded to «прошёл» or «не прошёл», the honest answer is that
/// the wording does not know it.
pub const FOUND_UNKNOWN: &str = "Чего требует этот итог — в отчёте не сказано; подробности ниже.";

/// A finding whose group decides the verdict.
pub const STAKE_BLOCKS: &str = "без этого работу принять нельзя";
/// A finding that does not block acceptance.
pub const STAKE_NUANCES: &str = "на будущее — принять не мешает";
/// A group this build does not know: no claim about its weight on the verdict.
pub const STAKE_NEUTRAL: &str = "на что это влияет — в подробностях";

/// The lead of the «не проверено» section: it is NOT a list of defects.
pub const UNCHECKED_LEAD: &str = "это не недостатки работы, а проверки, которых не хватило: пока их нет, часть картины неизвестна";
/// A blocked/unperformed check: the human can do nothing by hand.
pub const ACTION_BLOCKED: &str = "от человека здесь ничего не нужно: проверке не хватило условий; чтобы она прошла, нужен повторный прогон";
/// A gate probe that could not run.
pub const ACTION_GATE: &str = "нужен повторный прогон: проверка-шлюз не смогла выполниться";
/// A scored state without a score.
pub const ACTION_NO_SCORE: &str =
    "нужен повторный прогон: платформа получила состояние проверки без балла";
/// A state token this build does not know.
pub const ACTION_UNKNOWN: &str = "это дефект самого отчёта: платформа не знает такого состояния проверки — нужен повторный прогон аудита";
/// The signature the platform is waiting for.
pub const ACTION_PENDING: &str = "нужна твоя подпись — без неё итог не финализируется";
/// Nothing is required from the human for this datum.
pub const ACTION_NONE: &str = "здесь от тебя ничего не требуется";

/// The honest answer when a stored point names no reason at all.
pub const MISSING_UNRECORDED: &str = "причина в отчёте не записана";
/// The honest answer when a stored point names no site at all.
pub const SITE_UNNAMED: &str = "место не указано";

/// What the saved figure next to the assessment is, in words. The operator's
/// decision of 10.11 (REQ-SAY-02): the human sees BOTH the meaning and the
/// number, so the score is named for what it is instead of floating alone.
pub const SCORE_LEAD: &str = "сохранённый балл работы";

/// One row of the first layer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FirstLayerRow {
    /// The stored check id — the collapsed detail of this row, never its
    /// headline.
    pub id: String,
    /// What is wrong, in the stored words of the report.
    pub what: String,
    /// Where it was found; empty when the report named no site (the reader
    /// says [`SITE_UNNAMED`] instead of printing a hole).
    pub site: String,
    /// A finding: what it costs. An unchecked point: what to do about it.
    pub note: String,
    /// The failed/unperformed check's own stated reason, when the report
    /// stored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    /// The stored machine row (id · score · weight · importance · site) for
    /// the details layer.
    pub line: String,
    /// The collapsed proof layer, verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// The first layer of one stored form.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FirstLayerView {
    /// What the audit found, in one or two plain sentences.
    pub found: String,
    /// The saved overall score of the work, as stored — «сохранённый балл
    /// работы: 0.975». Read from `headline.score` by the one number formatter
    /// (`fmt_num`), never recomputed from the groups (REQ-SAY-02, v4).
    pub score_line: String,
    /// The findings the human is deciding about — decision items first, then
    /// the ones that do not block, in the stored order.
    pub findings: Vec<FirstLayerRow>,
    /// Checks that were not performed: NOT defects, kept apart.
    pub unchecked: Vec<FirstLayerRow>,
    /// The plain-words lead of the «не проверено» section.
    pub unchecked_lead: String,
}

/// The stored verdict token and counts in plain words. Built ONLY from values
/// the stored form already carries, so the sentence cannot contradict the
/// outcome the platform decided.
pub fn found(h: &HumanHeadline) -> String {
    match h.machine_verdict.as_str() {
        "PASS" if h.counts.blocks == 0 && h.counts.nuances == 0 => FOUND_PASS.to_string(),
        // A passed report that still carries a row below 1.0 is never called
        // «замечаний не нашёл»: the sentence must not contradict the rows the
        // human is about to read.
        "PASS" => FOUND_PASS_NUANCES.to_string(),
        "FAIL" => FOUND_FAIL.to_string(),
        "INCONCLUSIVE" if h.counts.unchecked > 0 => {
            format!("{FOUND_NOT_READY}{FOUND_NOT_READY_UNCHECKED}")
        }
        "INCONCLUSIVE" => FOUND_NOT_READY.to_string(),
        _ => FOUND_UNKNOWN.to_string(),
    }
}

/// What a finding of this group costs, derived from the SPLIT the platform
/// already made — no new severity scale.
pub fn stake(group: &str) -> &'static str {
    if group == GROUP_BLOCKS {
        STAKE_BLOCKS
    } else if group == GROUP_NUANCES {
        STAKE_NUANCES
    } else {
        STAKE_NEUTRAL
    }
}

/// What the human can do about a point of the «не проверено» group.
pub fn action(kind: &str) -> &'static str {
    if kind == ItemKind::Pending.as_str() {
        ACTION_PENDING
    } else if kind == ItemKind::Blocked.as_str() {
        ACTION_BLOCKED
    } else if kind == ItemKind::GateUnperformed.as_str() {
        ACTION_GATE
    } else if kind == ItemKind::NoScore.as_str() {
        ACTION_NO_SCORE
    } else if kind == ItemKind::Unknown.as_str() {
        ACTION_UNKNOWN
    } else {
        ACTION_NONE
    }
}

/// The stored site, or the honest sentence that none was named.
pub fn site_phrase(site: &str) -> String {
    if site.trim().is_empty() {
        SITE_UNNAMED.to_string()
    } else {
        site.to_string()
    }
}

/// The saved score line of the first layer: the figure is the headline's own
/// value, printed by the ONE number formatter — the same precision the details
/// layer uses, and nothing is re-computed from the group rows.
pub fn score_line(h: &HumanHeadline) -> String {
    format!("{SCORE_LEAD}: {}", super::fmt_num(h.score))
}

fn row(it: &HumanItem, note: &str, missing: Option<String>) -> FirstLayerRow {
    FirstLayerRow {
        id: it.id.clone(),
        what: it.summary.clone(),
        site: it.site.clone(),
        note: note.to_string(),
        missing,
        line: it.line.clone(),
        evidence: it.evidence.clone(),
    }
}

fn group_of<'a>(view: &'a HumanReportView, key: &str) -> Option<&'a HumanGroup> {
    view.groups.iter().find(|g| g.key == key)
}

fn non_empty<'a>(view: &'a HumanReportView, key: &str) -> Option<&'a HumanGroup> {
    group_of(view, key).filter(|g| !g.items.is_empty())
}

/// The first layer of one stored form: the found sentence, the findings and
/// the «не проверено» rows, read off the final groups.
pub fn of(view: &HumanReportView) -> FirstLayerView {
    let mut findings = Vec::new();
    for key in [GROUP_BLOCKS, GROUP_NUANCES] {
        if let Some(g) = non_empty(view, key) {
            for it in &g.items {
                findings.push(row(it, stake(g.key.as_str()), None));
            }
        }
    }
    let unchecked: Vec<FirstLayerRow> = non_empty(view, GROUP_UNCHECKED)
        .map(|g| g.items.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|it| {
            row(
                it,
                action(it.kind.as_str()),
                Some(
                    it.reason
                        .as_deref()
                        .filter(|r| !r.trim().is_empty())
                        .unwrap_or(MISSING_UNRECORDED)
                        .to_string(),
                ),
            )
        })
        .collect();
    FirstLayerView {
        found: found(&view.headline),
        score_line: score_line(&view.headline),
        findings,
        unchecked,
        unchecked_lead: UNCHECKED_LEAD.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::human_view::{HumanCounts, HumanItem};
    use devai_core::vocab::{CheckState, ItemKind};

    fn headline(verdict: &str, blocks: usize, nuances: usize, unchecked: usize) -> HumanHeadline {
        HumanHeadline {
            verdict: "·".into(),
            machine_verdict: verdict.into(),
            score: 0.5,
            theta: 0.8,
            basis: "b".into(),
            numbers: "S=0.500/1.00, порог 0.800".into(),
            counts: HumanCounts {
                blocks,
                nuances,
                anchors: 0,
                unchecked,
                passed: 0,
            },
        }
    }

    fn item(id: &str, kind: &str, summary: &str, site: &str) -> HumanItem {
        HumanItem {
            id: id.into(),
            kind: kind.into(),
            summary: summary.into(),
            line: format!("{id} · балл 0.900 · вес 0.100"),
            site: site.into(),
            score: Some(0.9),
            weight: Some(0.1),
            importance: Some(0.01),
            ratified_by: None,
            reason: None,
            evidence: vec![],
        }
    }

    fn group(key: &str, items: Vec<HumanItem>) -> HumanGroup {
        HumanGroup {
            key: key.into(),
            title: key.to_uppercase(),
            lead: String::new(),
            items,
        }
    }

    fn view(headline: HumanHeadline, groups: Vec<HumanGroup>) -> HumanReportView {
        HumanReportView {
            version: 2,
            lang: "ru".into(),
            headline,
            groups,
            blocked_weight: 0.0,
            max_blocked_weight: 0.2,
        }
    }

    /// The found sentence follows the STORED outcome — «годно» never appears
    /// where the run did not pass, and «пока не готово» only where there is no
    /// verdict yet.
    #[test]
    fn the_found_sentence_never_contradicts_the_stored_outcome() {
        assert_eq!(found(&headline("PASS", 0, 0, 0)), FOUND_PASS);
        assert_eq!(found(&headline("PASS", 0, 2, 0)), FOUND_PASS_NUANCES);
        assert_eq!(
            found(&headline("PASS", 1, 0, 0)),
            FOUND_PASS_NUANCES,
            "a passed report with a row below 1.0 is not «замечаний не нашёл»"
        );
        assert_eq!(found(&headline("FAIL", 3, 1, 1)), FOUND_FAIL);
        assert!(!found(&headline("FAIL", 3, 1, 1)).contains("годно"));
        assert_eq!(found(&headline("INCONCLUSIVE", 0, 0, 0)), FOUND_NOT_READY);
        let with_hole = found(&headline("INCONCLUSIVE", 0, 0, 2));
        assert!(with_hole.starts_with(FOUND_NOT_READY) && with_hole.contains("не выполнена"));
        // An unknown token is not rounded to a known outcome.
        let odd = found(&headline("SOMETHING_NEW", 0, 0, 0));
        assert_eq!(odd, FOUND_UNKNOWN);
        assert!(!odd.contains("годно") && !odd.contains("нельзя"));
    }

    /// The assessment SENTENCE says nothing numeric: the figure that stands
    /// next to it is a separate piece of the layer (REQ-SAY-02 v4: «рядом с
    /// оценкой стоит сохранённый балл работы»).
    #[test]
    fn the_found_sentence_carries_no_figures() {
        for verdict in ["PASS", "FAIL", "INCONCLUSIVE", "SOMETHING_NEW"] {
            let text = found(&headline(verdict, 2, 2, 2));
            assert!(
                !text.chars().any(|c| c.is_ascii_digit()),
                "{verdict}: a figure leaked into the sentence: {text}"
            );
        }
    }

    /// The score line is the stored headline score, printed by the one
    /// formatter — no re-scoring from groups, no second precision.
    #[test]
    fn the_score_line_is_the_stored_score_and_not_a_recount() {
        let h = headline("FAIL", 3, 1, 1);
        assert_eq!(score_line(&h), "сохранённый балл работы: 0.500");
        // Same three-decimal precision as every other figure in the form.
        let mut odd = h.clone();
        odd.score = 0.9754;
        assert_eq!(score_line(&odd), "сохранённый балл работы: 0.975");
        // It travels in the layer beside the sentence the human reads.
        let v = view(h, vec![]);
        let first = of(&v);
        assert_eq!(first.score_line, "сохранённый балл работы: 0.500");
    }

    /// Decision items come first, then the points that do not block; neither
    /// the machine id nor the machine row is lost, and evidence stays with the
    /// point.
    #[test]
    fn findings_are_plain_rows_with_the_machine_half_kept_for_the_details() {
        let mut blocking = item("blk/a", "floor", "не проверено ветвление", "src/a.rs:10");
        blocking.evidence = vec!["цитата".into()];
        let nuance = item("n/b", "scored", "мелкая придирка", "");
        let v = view(
            headline("FAIL", 1, 1, 0),
            vec![
                group(GROUP_BLOCKS, vec![blocking]),
                group(GROUP_NUANCES, vec![nuance]),
            ],
        );
        let first = of(&v);
        let ids: Vec<&str> = first.findings.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["blk/a", "n/b"], "decision items first");
        assert_eq!(first.findings[0].what, "не проверено ветвление");
        assert_eq!(first.findings[0].site, "src/a.rs:10");
        assert_eq!(first.findings[0].note, STAKE_BLOCKS);
        assert_eq!(first.findings[1].note, STAKE_NUANCES);
        assert_eq!(first.findings[0].evidence, vec!["цитата".to_string()]);
        assert!(
            first.findings[0].line.contains("балл 0.900"),
            "the stored machine row travels for the details layer"
        );
        // «не проверено» is a separate list, not folded into findings.
        assert!(first.unchecked.is_empty());
    }

    /// Unchecked points say WHAT was missing and WHAT to do — never mixed with
    /// the defects, and an unrecorded reason is said in words.
    #[test]
    fn unchecked_points_say_what_was_missing_and_what_to_do() {
        let mut blocked = item(
            "brk/x",
            ItemKind::Blocked.as_str(),
            "проверка не прошла",
            "",
        );
        blocked.reason = Some("probe could not run".into());
        let mut silent = item(
            "brk/y",
            ItemKind::Blocked.as_str(),
            "проверка не прошла",
            "",
        );
        silent.reason = Some("  ".into());
        let pending = item("p/p", ItemKind::Pending.as_str(), "ждёт подписи", "");
        let odd = item(
            "u/u",
            ItemKind::Unknown.as_str(),
            "неизвестное состояние",
            "",
        );
        let v = view(
            headline("INCONCLUSIVE", 0, 0, 4),
            vec![group(GROUP_UNCHECKED, vec![blocked, silent, pending, odd])],
        );
        let first = of(&v);
        assert!(first.findings.is_empty(), "no defect is invented here");
        assert_eq!(first.unchecked.len(), 4);
        assert_eq!(
            first.unchecked[0].missing.as_deref(),
            Some("probe could not run"),
            "the stored reason is what was missing"
        );
        assert_eq!(first.unchecked[0].note, ACTION_BLOCKED);
        assert_eq!(
            first.unchecked[1].missing.as_deref(),
            Some(MISSING_UNRECORDED),
            "an empty reason is said in words, not printed as a hole"
        );
        assert_eq!(first.unchecked[2].note, ACTION_PENDING);
        assert_eq!(first.unchecked[3].note, ACTION_UNKNOWN);
        assert_eq!(first.unchecked_lead, UNCHECKED_LEAD);
        assert!(
            first.found.contains("Часть проверок не выполнена"),
            "the found sentence names the hole: {}",
            first.found
        );
    }

    /// Anchors and fully passed points are not findings: the first layer lists
    /// what the human is deciding about and nothing else.
    #[test]
    fn anchors_are_not_findings() {
        let anchor = item(
            "anch/a",
            ItemKind::Anchor.as_str(),
            "закреплено решением",
            "",
        );
        let v = view(
            headline("PASS", 0, 0, 0),
            vec![group("anchors", vec![anchor])],
        );
        let first = of(&v);
        assert!(first.findings.is_empty() && first.unchecked.is_empty());
        assert_eq!(first.found, FOUND_PASS);
    }

    /// Every point of the stored form is reachable from the first layer (or is
    /// an anchor, which the details layer shows): nothing is dropped.
    #[test]
    fn every_stored_point_is_still_reachable() {
        let rows = [
            item("a", ItemKind::Floor.as_str(), "a", ""),
            item("b", ItemKind::Scored.as_str(), "b", ""),
            item("c", CheckState::Blocked.as_str(), "c", ""),
        ];
        let v = view(
            headline("FAIL", 1, 1, 1),
            vec![
                group(GROUP_BLOCKS, vec![rows[0].clone()]),
                group(GROUP_NUANCES, vec![rows[1].clone()]),
                group(GROUP_UNCHECKED, vec![rows[2].clone()]),
            ],
        );
        let first = of(&v);
        let mut seen: Vec<&str> = first.findings.iter().map(|r| r.id.as_str()).collect();
        seen.extend(first.unchecked.iter().map(|r| r.id.as_str()));
        assert_eq!(seen, vec!["a", "b", "c"]);
    }
}
