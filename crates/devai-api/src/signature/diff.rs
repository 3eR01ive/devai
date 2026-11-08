//! The human field diff of two spec versions (REQ-SIGN-13). Comparison is
//! made on PARSED values, so JSON reformatting never reads as a change.
//!
//! A side whose body never became a document is REFUSED, with the side named
//! and the cause carried in `note` + `refusal` (rework R3/R8): the old code
//! fell back to the business description on both sides and printed a diff of
//! paragraph/requirements that did not exist.

use super::spec::{ParsedSpec, RequirementView};
use super::types::{DiffLine, DiffRefusal, SpecDiff};

/// The field diff of two parsed specs (REQ-SIGN-13) — or the honest refusal
/// when either side's body was not a readable spec document.
pub fn diff_specs(previous: &ParsedSpec, current: &ParsedSpec) -> SpecDiff {
    if let Some(refused) = refusal_of(previous, current) {
        return refused;
    }
    SpecDiff {
        paragraph: diff_paragraph(
            previous.human_paragraph.as_deref().unwrap_or(""),
            current.human_paragraph.as_deref().unwrap_or(""),
        ),
        requirements: diff_requirements(previous, current),
        refusal: None,
        note: None,
    }
}

/// The empty diff carrying the causes: no line is invented on a refused
/// comparison, and the sentence names every failing side.
fn refusal_of(previous: &ParsedSpec, current: &ParsedSpec) -> Option<SpecDiff> {
    let previous_problem = previous.body_problem.as_ref();
    let current_problem = current.body_problem.as_ref();
    if previous_problem.is_none() && current_problem.is_none() {
        return None;
    }
    let mut sides = Vec::new();
    if let Some(problem) = current_problem {
        sides.push(format!("текущая версия не читается — {}", problem.reason()));
    }
    if let Some(problem) = previous_problem {
        sides.push(format!("прошлая версия не читается — {}", problem.reason()));
    }
    Some(SpecDiff {
        paragraph: Vec::new(),
        requirements: Vec::new(),
        refusal: Some(DiffRefusal {
            previous: previous_problem.cloned(),
            current: current_problem.cloned(),
        }),
        note: Some(format!("сравнить не удалось: {}", sides.join("; "))),
    })
}

/// Requirement lines in the CURRENT order, then the removed requirements.
fn diff_requirements(previous: &ParsedSpec, current: &ParsedSpec) -> Vec<DiffLine> {
    let mut out = Vec::new();
    for req in &current.requirements {
        let old = previous.requirements.iter().find(|p| p.id == req.id);
        out.extend(requirement_lines(old, req));
    }
    out.extend(removed_requirements(previous, current));
    out
}

fn requirement_lines(old: Option<&RequirementView>, req: &RequirementView) -> Vec<DiffLine> {
    match old {
        None => added_requirement(req),
        Some(old) if old == req => vec![line(" ", &label_of(&req.id), &req.statement)],
        Some(old) => changed_requirement(old, req),
    }
}

fn added_requirement(req: &RequirementView) -> Vec<DiffLine> {
    let mut out = vec![line("+", &label_of(&req.id), &req.statement)];
    let ac_label = format!("{} · AC", label_of(&req.id));
    let constraint_label = format!("{} · ограничение", label_of(&req.id));
    out.extend(
        req.acceptance_criteria
            .iter()
            .map(|ac| line("+", &ac_label, ac)),
    );
    out.extend(
        req.constraints
            .iter()
            .map(|c| line("+", &constraint_label, c)),
    );
    out
}

fn changed_requirement(old: &RequirementView, req: &RequirementView) -> Vec<DiffLine> {
    let mut out = Vec::new();
    if old.statement != req.statement {
        out.push(line("-", &label_of(&req.id), &old.statement));
        out.push(line("+", &label_of(&req.id), &req.statement));
    }
    let ac_label = format!("{} · AC", label_of(&req.id));
    let constraint_label = format!("{} · ограничение", label_of(&req.id));
    out.extend(removed_values(
        &old.acceptance_criteria,
        &req.acceptance_criteria,
        &ac_label,
    ));
    out.extend(added_values(
        &old.acceptance_criteria,
        &req.acceptance_criteria,
        &ac_label,
    ));
    out.extend(removed_values(
        &old.constraints,
        &req.constraints,
        &constraint_label,
    ));
    out.extend(added_values(
        &old.constraints,
        &req.constraints,
        &constraint_label,
    ));
    out
}

fn removed_values(old: &[String], new: &[String], label: &str) -> Vec<DiffLine> {
    old.iter()
        .filter(|v| !new.contains(v))
        .map(|v| line("-", label, v))
        .collect()
}

fn added_values(old: &[String], new: &[String], label: &str) -> Vec<DiffLine> {
    new.iter()
        .filter(|v| !old.contains(v))
        .map(|v| line("+", label, v))
        .collect()
}

fn removed_requirements(previous: &ParsedSpec, current: &ParsedSpec) -> Vec<DiffLine> {
    previous
        .requirements
        .iter()
        .filter(|old| !current.requirements.iter().any(|r| r.id == old.id))
        .map(|old| line("-", &label_of(&old.id), &old.statement))
        .collect()
}

fn label_of(id: &str) -> String {
    if id.is_empty() {
        "требование".to_string()
    } else {
        id.to_string()
    }
}

fn line(mark: &str, label: &str, text: &str) -> DiffLine {
    DiffLine {
        mark: mark.to_string(),
        label: label.to_string(),
        text: text.to_string(),
    }
}

/// Line diff with unchanged context (small LCS over lines — the paragraph is
/// prose, line granularity is the human unit).
fn diff_paragraph(old: &str, new: &str) -> Vec<DiffLine> {
    let a = lines_of(old);
    let b = lines_of(new);
    let lcs = lcs_table(&a, &b);
    walk(&a, &b, &lcs)
}

fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.lines().collect()
    }
}

fn lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    lcs
}

fn walk(a: &[&str], b: &[&str], lcs: &[Vec<usize>]) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(line(" ", "абзац", a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(line("-", "абзац", a[i]));
            i += 1;
        } else {
            out.push(line("+", "абзац", b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| line("-", "абзац", l)));
    out.extend(b[j..].iter().map(|l| line("+", "абзац", l)));
    out
}
