//! Presentation of a STORED quality report: which artifact row is shown, how
//! its body is rendered, and whether it is still the last word (REQ-QH-05/06).
//!
//! Everything here is a pure function over rows the caller has already read
//! from the store — no store handle, no read-time re-derivation of the report
//! content. The API layer (`devai-api`) only fetches rows and blobs and hands
//! them in; the decisions «what is shown», «what is the freshness marker» and
//! «how is the body laid out» live with the human form itself.

use devai_core::types::{
    Artifact, ArtifactKind, ArtifactStatus, ExecutionStatus, RoleExecution, Task, TaskState,
    Timestamp, short_id,
};
use serde::Serialize;
use serde_json::Value;

use super::{HumanReportView, render_text};

/// The role whose executions are the re-audit the freshness marker may name
/// (rework R10: a slug, not a literal at the filter site).
pub const AUDITOR_ROLE: &str = "quality-auditor";

/// REQ-005 / REQ-QH-06: status-only selection. CURRENT wins; when there is
/// none, the newest row (by seq) is still selected whatever its status — for
/// a task that is not terminal it is then shown under an explicit «устарел»
/// marker (see [`freshness`]), never as the current decision and never hidden
/// as «QA has not run».
///
/// rework R2: this is the ONE selection rule. Both readers — the presented
/// report (`mutations::presented_quality`) and the findings quoted into the
/// implementer prompt (`context::quality_findings`) — call it, so they can
/// never disagree on which report is «the» report (STALE + SUPERSEDED used
/// to diverge: the panel showed one row, the prompt quoted another).
pub fn select_report(arts: &[Artifact]) -> Option<&Artifact> {
    arts.iter()
        .filter(|a| a.kind == ArtifactKind::QualityReport)
        .max_by_key(|a| (a.status == ArtifactStatus::Current, a.seq))
}

/// What the presented text was rendered FROM — the honest outcome of the
/// read (REQ-QH-06): the report's own stored form, its legacy flat text, a
/// damaged form (announced in the text, flat text kept if there is one), or
/// nothing at all. A structurally broken form is never silently swapped for
/// the flat text: the reader must be able to tell the two apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderSource {
    /// quality-report@5+: the stored human form.
    HumanForm,
    /// A pre-@5 body: its own stored `report_text`, byte for byte.
    FlatText,
    /// A `human_view` that does not parse: marker + flat text (if any).
    CorruptForm,
    /// The blob itself does not parse as JSON: marker, never «no report»
    /// and never the generic «assembled before @2» placeholder (rework R4).
    CorruptBody,
    /// Neither a form nor a flat text (pre-@2 body).
    Absent,
}

impl RenderSource {
    /// The token the API publishes with the view, so a consumer can tell a
    /// rendered form from a fallback without sniffing the text.
    pub const fn as_str(self) -> &'static str {
        match self {
            RenderSource::HumanForm => "human_form",
            RenderSource::FlatText => "flat_text",
            RenderSource::CorruptForm => "corrupt_form",
            RenderSource::CorruptBody => "corrupt_body",
            RenderSource::Absent => "absent",
        }
    }
}

/// One rendered report body plus the distinguishable source it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedReport {
    pub text: String,
    pub source: RenderSource,
}

/// The text of a report whose BLOB does not parse as JSON at all (rework R4):
/// a typed, distinguishable outcome instead of an empty success or the
/// «assembled before @2» placeholder that used to stand in for it.
pub fn corrupt_body_text(artifact_short: &str, err: &str) -> String {
    format!(
        "⚠ отчёт качества {artifact_short} повреждён: тело блоба не разбирается как JSON ({err}) — рендеринга нет, отчёт нельзя предъявлять как решение"
    )
}

/// The text of ONE stored report body. Pure layout: a stored form is laid out
/// by `render_text` (grouping, order and numbers are read, never recomputed);
/// a legacy body is quoted byte for byte; a damaged form is announced.
pub fn render_stored_report(v: &Value) -> RenderedReport {
    let flat = v.get("report_text").and_then(|t| t.as_str());
    if let Some(form) = v.get("human_view") {
        return match serde_json::from_value::<HumanReportView>(form.clone()) {
            Ok(form) => RenderedReport {
                text: render_text(&form),
                source: RenderSource::HumanForm,
            },
            Err(_) => RenderedReport {
                text: match flat {
                    Some(flat) => {
                        format!("⚠ форма отчёта повреждена, показан плоский текст\n\n{flat}")
                    }
                    None => "⚠ форма отчёта повреждена — рендеринга нет".to_string(),
                },
                source: RenderSource::CorruptForm,
            },
        };
    }
    match flat {
        Some(flat) => RenderedReport {
            text: flat.to_string(),
            source: RenderSource::FlatText,
        },
        None => RenderedReport {
            text: "(report assembled before @2 — no rendering)".to_string(),
            source: RenderSource::Absent,
        },
    }
}

/// Why the report on screen is (or is not) the last word — raw store facts:
/// the task phase, the stored artifact status, and the auditor execution.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ReportFreshness {
    pub stale: bool,
    /// Ready-to-show marker line; empty when the report is current.
    pub marker: String,
    pub report_seq: u32,
    pub report_status: String,
    pub task_state: String,
    pub execution: Option<String>,
    pub execution_status: Option<String>,
}

/// One presented report: the text a human signs, the distinguishable source it
/// was rendered from, and why it is (or is not) the last word.
#[derive(Debug, Clone)]
pub struct PresentedReport {
    pub text: String,
    pub source: RenderSource,
    pub freshness: ReportFreshness,
}

impl PresentedReport {
    /// The text-only view of the same presentation: the freshness marker and
    /// the «это НЕ текущее решение» label are PREPENDED when the report is
    /// outdated, so a consumer that only takes a string cannot silently show
    /// a stale report as the decision (REQ-QH-05). Channels that render the
    /// marker themselves use `text` + `freshness` instead.
    pub fn text_with_freshness(&self) -> String {
        if !self.freshness.stale {
            return self.text.clone();
        }
        format!(
            "{}\n(показан отчёт v{} в статусе {} — это НЕ текущее решение)\n\n{}",
            self.freshness.marker,
            self.freshness.report_seq,
            self.freshness.report_status,
            self.text
        )
    }
}

/// The timestamp a freshness marker prints: UTC, second precision, no
/// timezone math — the stored fact, formatted once.
fn fmt_ts(t: &Timestamp) -> String {
    t.format("%Y-%m-%d %H:%M:%SZ").to_string()
}

/// REQ-QH-05: freshness as a FACT of the store — task phase, stored artifact
/// status, role executions. Nothing is inferred from the report body, and the
/// report content is not touched.
pub fn freshness(task: &Task, a: &Artifact, execs: &[RoleExecution]) -> ReportFreshness {
    let reworking = task.state == TaskState::Rework || task.return_state == Some(TaskState::Rework);
    let stale = (reworking || a.status != ArtifactStatus::Current) && !task.state.is_terminal();
    let execs: Vec<&RoleExecution> = execs
        .iter()
        .filter(|e| e.role_slug == AUDITOR_ROLE)
        .collect();
    let running = execs
        .iter()
        .filter(|e| e.status == ExecutionStatus::Running)
        .max_by_key(|e| e.started_at);
    let last = execs.iter().max_by_key(|e| e.started_at);
    if !stale {
        return current_report(task, a, last.copied());
    }
    ReportFreshness {
        stale: true,
        marker: format!(
            "⚠ отчёт v{} УСТАРЕЛ — задача в {}: {}",
            a.seq,
            task.state.as_str(),
            stale_tail(running.copied(), last.copied())
        ),
        report_seq: a.seq,
        report_status: a.status.as_str().to_string(),
        task_state: task.state.as_str().to_string(),
        execution: last.map(|e| e.id.to_string()),
        execution_status: last.map(|e| e.status.as_str().to_string()),
    }
}

/// A report that is still the last word: no marker, but the same raw facts.
fn current_report(task: &Task, a: &Artifact, last: Option<&RoleExecution>) -> ReportFreshness {
    ReportFreshness {
        stale: false,
        marker: String::new(),
        report_seq: a.seq,
        report_status: a.status.as_str().to_string(),
        task_state: task.state.as_str().to_string(),
        execution: last.map(|e| e.id.to_string()),
        execution_status: last.map(|e| e.status.as_str().to_string()),
    }
}

/// What is happening to the outdated report, as stored: one branch per
/// store fact (a live re-audit / a return without one / an unfinished run /
/// nothing launched). Texts are unchanged from the previous ladder.
fn stale_tail(running: Option<&RoleExecution>, last: Option<&RoleExecution>) -> String {
    if let Some(e) = running {
        // «в работе» is claimed ONLY when a live RUNNING execution exists.
        return reaudit_running(e);
    }
    match last {
        // The newest auditor run is the COMPLETED one that produced the very
        // report on screen: there is no re-audit in flight, and calling that
        // run «не завершён» would invent one (rework R1).
        Some(e) if e.status == ExecutionStatus::Completed => reaudit_not_launched(e),
        Some(e) => reaudit_unfinished(e),
        None => "ре-аудит ещё не запущен".to_string(),
    }
}

fn reaudit_running(e: &RoleExecution) -> String {
    format!(
        "готовится ре-аудит (execution {}, в работе)",
        short_id(&e.id)
    )
}

fn reaudit_not_launched(e: &RoleExecution) -> String {
    format!(
        "возврат без ре-аудита; последний прогон {} COMPLETED {}",
        short_id(&e.id),
        fmt_ts(e.finished_at.as_ref().unwrap_or(&e.started_at))
    )
}

fn reaudit_unfinished(e: &RoleExecution) -> String {
    format!(
        "ре-аудит не завершён (execution {}, {})",
        short_id(&e.id),
        e.status.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use devai_core::types::{ContractRef, DataCategory, new_id};

    /// One artifact row of the shape the store hands in — the fields the
    /// selection rule reads are the ones set here.
    fn art(kind: ArtifactKind, seq: u32, status: ArtifactStatus) -> Artifact {
        Artifact {
            id: new_id(),
            task_id: new_id(),
            kind,
            name: format!("{}-v{seq}", kind.as_str().to_lowercase()),
            seq,
            category: DataCategory::Artifact,
            status,
            sha256: format!("sha-{seq}"),
            size: 1,
            media_type: "application/json".into(),
            contract: Some(ContractRef {
                name: "quality-report".into(),
                version: 5,
            }),
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
            created_at: Utc::now(),
        }
    }

    /// R2 (rework): the ONE selection rule answers for SUPERSEDED too — a
    /// CURRENT row always wins, and without one the newest row is selected
    /// whatever its status, so both readers quote the same report.
    #[test]
    fn a_superseded_report_is_selected_by_the_same_rule() {
        let arts = vec![
            art(ArtifactKind::QualityReport, 1, ArtifactStatus::Stale),
            art(ArtifactKind::QualityReport, 2, ArtifactStatus::Superseded),
        ];
        let picked = select_report(&arts).expect("a report is selected");
        assert_eq!(
            (picked.seq, picked.status),
            (2, ArtifactStatus::Superseded),
            "no CURRENT: the newest row is shown (it is marked stale on screen)"
        );

        // A CURRENT row wins even when an older SUPERSEDED one still exists.
        let arts = vec![
            art(ArtifactKind::QualityReport, 1, ArtifactStatus::Superseded),
            art(ArtifactKind::QualityReport, 2, ArtifactStatus::Current),
        ];
        let picked = select_report(&arts).expect("a report is selected");
        assert_eq!(picked.status, ArtifactStatus::Current);

        // Only current: highest seq.
        let arts = vec![
            art(ArtifactKind::QualityReport, 1, ArtifactStatus::Current),
            art(ArtifactKind::QualityReport, 2, ArtifactStatus::Current),
        ];
        assert_eq!(select_report(&arts).unwrap().seq, 2);

        // No quality report at all: honest «none», never another kind's row.
        let arts = vec![art(ArtifactKind::Patch, 1, ArtifactStatus::Current)];
        assert!(select_report(&arts).is_none());
    }

    /// The store row shape the freshness fact reads (role, status, times).
    fn exec(status: ExecutionStatus, started: chrono::DateTime<Utc>) -> RoleExecution {
        RoleExecution {
            id: new_id(),
            task_id: new_id(),
            role_slug: AUDITOR_ROLE.into(),
            role_version: 1,
            agent_name: None,
            status,
            inputs: vec![],
            summary: None,
            error: None,
            started_at: started,
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
        }
    }

    fn task_in(state: TaskState) -> Task {
        let mut t = Task::new("freshness", "unit", None, Utc::now());
        t.state = state;
        t
    }

    /// REQ-QH-05: the marker names a STORE fact, never a guess. This unit
    /// pins the three non-live branches — nothing launched, a last run that
    /// is neither RUNNING nor COMPLETED («не завершён», never «в работе»),
    /// and a COMPLETED last run (a return without a re-audit). The live
    /// RUNNING branch is covered end-to-end through the API.
    #[test]
    fn the_freshness_marker_names_only_store_facts() {
        let now = Utc::now();
        let a = art(ArtifactKind::QualityReport, 1, ArtifactStatus::Stale);

        let none = freshness(&task_in(TaskState::Rework), &a, &[]);
        assert!(none.stale && none.marker.contains("ре-аудит ещё не запущен"));
        assert_eq!(none.execution, None);

        let failed = exec(ExecutionStatus::Failed, now - chrono::Duration::seconds(5));
        let unfinished = freshness(&task_in(TaskState::Rework), &a, &[failed]);
        assert!(
            unfinished.marker.contains("ре-аудит не завершён")
                && unfinished.marker.contains("FAILED"),
            "{}",
            unfinished.marker
        );
        assert_eq!(unfinished.execution_status.as_deref(), Some("FAILED"));

        let done = exec(ExecutionStatus::Completed, now);
        let no_reaudit = freshness(&task_in(TaskState::Rework), &a, &[done]);
        assert!(
            no_reaudit.marker.contains("возврат без ре-аудита"),
            "a COMPLETED last run is not an unfinished re-audit: {}",
            no_reaudit.marker
        );
    }

    /// The two ways a report IS the last word: CURRENT on a non-terminal
    /// task, or any status once the task is terminal — no marker is invented
    /// in either case (the same raw facts are still carried).
    #[test]
    fn a_current_report_on_a_live_task_carries_no_marker() {
        let a = art(ArtifactKind::QualityReport, 1, ArtifactStatus::Current);
        let fresh = freshness(&task_in(TaskState::Qa), &a, &[]);
        assert!(!fresh.stale);
        assert!(fresh.marker.is_empty());
        assert_eq!(fresh.report_seq, 1);
        assert_eq!(fresh.task_state, "QA");

        // A settled task does not present a «stale» warning: nothing is
        // being re-decided any more (same status, no marker).
        let settled = freshness(
            &task_in(TaskState::Accepted),
            &art(ArtifactKind::QualityReport, 1, ArtifactStatus::Stale),
            &[],
        );
        assert!(!settled.stale);
        assert!(settled.marker.is_empty());
    }
}
