//! The QA gate's PUBLIC availability read (REQ-QG-08/09): whether the operator
//! may accept or return the task, and WHY not — in the platform's own words,
//! not re-derived by the window.
//!
//! The conditions mirror `workflow::accept` / `workflow::return_to_rework`
//! (phase, current quality report, open blocking dependencies) and add the
//! one presentation fact REQ-QG-09 made explicit: a RUNNING audit is not a
//! human wait — while the auditor works, neither action is offered. «Works»
//! means the owner of the RUNNING row is not provably dead: a reboot or a
//! killed runner leaves an orphan nobody can close from the window, and
//! calling that an audit forever would take the gate away from the operator
//! (rework R1).
//!
//! Nothing here writes and nothing is cached: the read is a pure function of
//! rows the caller already loaded from the store.

use devai_core::process::Process;
use devai_core::types::{
    Artifact, ArtifactKind, ArtifactStatus, Decision, ExecutionStatus, RoleExecution, Task,
    TaskDependency, TaskState,
};
use devai_core::workflow;
use devai_runtime::liveness::{self, Liveness};
use serde::Serialize;

/// A quality audit is running: the gate is not waiting for a human yet.
pub const AUDIT_RUNNING: &str = "audit_running";
/// There is no CURRENT quality report to accept.
pub const NO_CURRENT_REPORT: &str = "no_current_report";
/// Open `blocks` dependencies hold the acceptance (no override covers them).
pub const OPEN_BLOCKERS: &str = "open_blockers";

/// Every code the read may return — one dictionary, pinned by a test.
pub const DENIAL_CODES: &[&str] = &[AUDIT_RUNNING, NO_CURRENT_REPORT, OPEN_BLOCKERS];

/// The roles that may run ONLY in QA are the audit roles (REQ-QG-09). The
/// roster is DERIVED from the declared process — the one place a role is
/// bound to a state (REQ-AR-09 AC1, rework R1): a role the declaration
/// confines to QA is an audit role by that very fact, so a third QA role
/// declared there cannot silently lose its «идёт аудит» mark, and no code
/// list decides who audits.
pub fn is_audit_role(process: &Process, slug: &str) -> bool {
    process
        .roles_confined_to(TaskState::Qa.as_str())
        .contains(&slug)
}

/// The label the active-executions view prints for a live audit (REQ-QG-09
/// AC3: «идёт аудит» is not «ждёт вашего решения»).
pub const AUDIT_RUNNING_LABEL: &str = "идёт аудит";

/// One typed refusal: a stable code for branching plus the platform sentence
/// for the operator. The window prints the sentence verbatim and never
/// rebuilds it from task state.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct QualityGateDenial {
    pub code: String,
    pub text: String,
}

impl QualityGateDenial {
    fn new(code: &str, text: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            text: text.into(),
        }
    }
}

/// The whole availability fact of the QA gate. `None` outside QA: the actions
/// do not exist there (REQ-QG-01 AC3), so there is nothing to offer.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct QualityGateView {
    pub audit_running: bool,
    /// The CURRENT quality report's artifact id, when one exists.
    pub current_report: Option<String>,
    /// Open blocking dependency ids the acceptance refuses on.
    pub open_blockers: Vec<String>,
    pub may_accept: bool,
    pub accept_denied: Option<QualityGateDenial>,
    pub may_return: bool,
    pub return_denied: Option<QualityGateDenial>,
}

/// Is this row the audit the gate must wait for? Status alone is not enough
/// (rework R1): a reboot or a killed runner leaves a RUNNING row nobody owns,
/// and the liveness doctrine resolves such an orphan only by a human
/// `exec fail|cancel` — so the gate must not wait for it forever. A provably
/// DEAD owner is «not running». `Unknown` (no owner recorded, another host,
/// no `/proc`) still counts as running: an unanswerable question is never
/// promoted to death, and closing a live audit's gate early would be worse.
pub fn is_live_audit(process: &Process, e: &RoleExecution) -> bool {
    e.status == ExecutionStatus::Running
        && is_audit_role(process, &e.role_slug)
        && liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start)
            != Liveness::Dead
}

/// Whether a quality audit is live right now.
pub fn audit_running(process: &Process, executions: &[RoleExecution]) -> bool {
    executions.iter().any(|e| is_live_audit(process, e))
}

/// The public gate read: `Some` in QA, `None` anywhere else.
///
/// The two acts are judged INDEPENDENTLY (REQ-QG-08 AC4): a missing report
/// blocks acceptance but never hides the return.
pub fn quality_gate(
    process: &Process,
    task: &Task,
    artifacts: &[Artifact],
    deps: &[TaskDependency],
    decisions: &[Decision],
    executions: &[RoleExecution],
) -> Option<QualityGateView> {
    if task.state != TaskState::Qa {
        return None;
    }
    let audit_running = audit_running(process, executions);
    let current_report = artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::QualityReport && a.status == ArtifactStatus::Current)
        .map(|a| a.id.to_string());
    let open_blockers = workflow::effective_blockers(task, deps, decisions);
    let (may_accept, accept_denied) = accept_right(audit_running, &current_report, &open_blockers);
    let (may_return, return_denied) = return_right(audit_running);
    Some(QualityGateView {
        audit_running,
        current_report,
        open_blockers,
        may_accept,
        accept_denied,
        may_return,
        return_denied,
    })
}

/// The acceptance conjuncts in the order the operation checks them: a live
/// audit first (REQ-QG-09), then the dependency gate, then the current report.
fn accept_right(
    audit_running: bool,
    current_report: &Option<String>,
    open_blockers: &[String],
) -> (bool, Option<QualityGateDenial>) {
    if audit_running {
        return (false, Some(audit_denial()));
    }
    if !open_blockers.is_empty() {
        return (
            false,
            Some(QualityGateDenial::new(
                OPEN_BLOCKERS,
                format!(
                    "открытые блокирующие зависимости: {}",
                    open_blockers.join(", ")
                ),
            )),
        );
    }
    if current_report.is_none() {
        return (
            false,
            Some(QualityGateDenial::new(
                NO_CURRENT_REPORT,
                "нет текущего отчёта качества — принять нечего",
            )),
        );
    }
    (true, None)
}

/// The return right is the QA phase alone — except that a live audit is not a
/// human wait (REQ-QG-09 AC1: the gate offers no action while the auditor
/// works).
fn return_right(audit_running: bool) -> (bool, Option<QualityGateDenial>) {
    if audit_running {
        return (false, Some(audit_denial()));
    }
    (true, None)
}

fn audit_denial() -> QualityGateDenial {
    QualityGateDenial::new(
        AUDIT_RUNNING,
        format!("{AUDIT_RUNNING_LABEL}: принять или вернуть можно после его завершения"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use devai_core::types::{DataCategory, Decision, RoleExecution, new_id};

    /// The factory declaration — the role/stage authority these tests exercise.
    fn proc() -> Process {
        Process::factory().clone()
    }

    fn qa_task() -> Task {
        let mut t = Task::new("gate", "unit", None, Utc::now());
        t.state = TaskState::Qa;
        t
    }

    fn report(status: ArtifactStatus) -> Artifact {
        Artifact {
            id: new_id(),
            task_id: new_id(),
            kind: ArtifactKind::QualityReport,
            name: "quality-report.json".into(),
            seq: 1,
            category: DataCategory::Artifact,
            status,
            sha256: "0".repeat(64),
            size: 2,
            media_type: "application/json".into(),
            contract: None,
            created_by: "auditor".into(),
            execution_id: None,
            depends_on: vec![],
            created_at: Utc::now(),
        }
    }

    fn execution(status: ExecutionStatus) -> RoleExecution {
        RoleExecution {
            id: new_id(),
            task_id: new_id(),
            role_slug: "quality-auditor".into(),
            role_version: 1,
            agent_name: None,
            status,
            inputs: vec![],
            summary: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: None,
            run_id: None,
        }
    }

    fn blocking_dep(task: &Task, blocker_id: devai_core::types::Id) -> TaskDependency {
        TaskDependency {
            id: new_id(),
            task_id: task.id,
            task_state: TaskState::Qa,
            task_title: "gate".into(),
            depends_on_id: blocker_id,
            depends_on_state: TaskState::Implementation,
            depends_on_title: "blocker".into(),
            task_display: "gate".into(),
            depends_on_display: "blocker".into(),
            kind: devai_core::types::DependencyKind::Blocks,
            actor: "user".into(),
            created_at: Utc::now(),
        }
    }

    /// The gate exists only at QA; elsewhere neither act is offered.
    #[test]
    fn outside_qa_there_is_no_gate() {
        let mut t = qa_task();
        t.state = TaskState::Accepted;
        assert!(quality_gate(&proc(), &t, &[], &[], &[], &[]).is_none());
    }

    /// The happy path: current report, no blockers, no live audit → both acts.
    #[test]
    fn a_current_report_opens_both_acts() {
        let g = quality_gate(
            &proc(),
            &qa_task(),
            &[report(ArtifactStatus::Current)],
            &[],
            &[],
            &[execution(ExecutionStatus::Completed)],
        )
        .unwrap();
        assert!(g.may_accept && g.may_return);
        assert!(g.accept_denied.is_none() && g.return_denied.is_none());
        assert!(g.current_report.is_some());
    }

    /// Acceptance needs a CURRENT report; the return does not — one denied act
    /// never hides the other (REQ-QG-08 AC4).
    #[test]
    fn a_missing_current_report_denies_acceptance_only() {
        for artifacts in [vec![], vec![report(ArtifactStatus::Stale)]] {
            let g = quality_gate(&proc(), &qa_task(), &artifacts, &[], &[], &[]).unwrap();
            assert!(!g.may_accept);
            assert_eq!(
                g.accept_denied.as_ref().map(|d| d.code.as_str()),
                Some(NO_CURRENT_REPORT)
            );
            assert!(g.may_return, "return stays available");
        }
    }

    /// A RUNNING audit is not a human wait: both acts are off, with the same
    /// typed reason (REQ-QG-09 AC1).
    #[test]
    fn a_live_audit_withholds_both_acts() {
        let g = quality_gate(
            &proc(),
            &qa_task(),
            &[report(ArtifactStatus::Current)],
            &[],
            &[],
            &[execution(ExecutionStatus::Running)],
        )
        .unwrap();
        assert!(g.audit_running);
        assert!(!g.may_accept && !g.may_return);
        assert_eq!(
            g.accept_denied.as_ref().map(|d| d.code.as_str()),
            Some(AUDIT_RUNNING)
        );
        assert_eq!(
            g.return_denied.as_ref().map(|d| d.code.as_str()),
            Some(AUDIT_RUNNING)
        );
        assert!(g.accept_denied.unwrap().text.contains(AUDIT_RUNNING_LABEL));
    }

    /// Rework R1: a RUNNING row whose owner is provably dead is an ORPHAN, not
    /// an audit. The gate must behave exactly as if no run existed — otherwise
    /// a reboot locks the task out of the window for good (the only resolution
    /// of an orphan is a human `exec fail|cancel`).
    #[test]
    fn an_orphaned_audit_row_does_not_block_the_gate() {
        let Some(host) = liveness::current_host() else {
            return; // no `/proc`: nothing is provably dead, nothing to assert
        };
        let mut orphan = execution(ExecutionStatus::Running);
        orphan.owner_pid = Some(i64::MAX);
        orphan.owner_host = Some(host);
        let g = quality_gate(
            &proc(),
            &qa_task(),
            &[report(ArtifactStatus::Current)],
            &[],
            &[],
            &[orphan],
        )
        .unwrap();
        assert!(!g.audit_running, "a dead owner is not a live audit");
        assert!(g.may_accept && g.may_return);
        assert_eq!(g.accept_denied, None);
    }

    /// Rework R1/R4: the audit roster is the DECLARATION's QA-only rule, not
    /// a second list in code. Under the factory declaration `quality-auditor`
    /// is the QA-only role, so it answers «audit»; roles declared elsewhere (or
    /// not declared at all) do not. Editing the declaration — not the code —
    /// moves the mark: the roster follows `process.yaml`.
    #[test]
    fn the_audit_roster_follows_the_declaration() {
        let factory = proc();
        assert!(is_audit_role(&factory, "quality-auditor"));
        for slug in [
            "implementation-agent",
            "requirements-analyst",
            "quality-aggregator",
            "unknown-role",
        ] {
            assert!(
                !is_audit_role(&factory, slug),
                "`{slug}` is not the declared executor of QA"
            );
        }

        // The same code, another declaration: QA's executor is the aggregator.
        let mut edited = factory;
        for st in &mut edited.states {
            if st.state == TaskState::Qa.as_str() {
                st.role = Some("quality-aggregator".into());
            }
        }
        assert!(is_audit_role(&edited, "quality-aggregator"));
        assert!(
            !is_audit_role(&edited, "quality-auditor"),
            "the declaration, not a code roster, decides who audits"
        );
    }

    /// Open blockers hold the acceptance and are enumerated; return is
    /// untouched. The override decision is the only release (the same rule the
    /// operation applies).
    #[test]
    fn open_blockers_deny_acceptance_with_the_ids() {
        let task = qa_task();
        let blocker_id = new_id();
        let dep = blocking_dep(&task, blocker_id);
        let arts = [report(ArtifactStatus::Current)];
        let g = quality_gate(&proc(), &task, &arts, &[dep.clone()], &[], &[]).unwrap();
        assert!(!g.may_accept && g.may_return);
        assert_eq!(g.open_blockers, vec![blocker_id.to_string()]);
        assert!(
            g.accept_denied
                .as_ref()
                .unwrap()
                .text
                .contains(&blocker_id.to_string())
        );

        // A covering dependency_override releases the acceptance.
        let override_decision = Decision {
            id: new_id(),
            task_id: task.id,
            kind: "dependency_override".into(),
            verdict: devai_core::types::Verdict::Approved,
            actor: "guerrero".into(),
            rationale: Some("blocker accepted".into()),
            refs: vec![blocker_id.to_string()],
            created_at: Utc::now(),
            payload: serde_json::Value::Null,
        };
        let g = quality_gate(&proc(), &task, &arts, &[dep], &[override_decision], &[]).unwrap();
        assert!(g.may_accept, "a covering override releases the gate");
    }

    /// Every code produced is in the one dictionary.
    #[test]
    fn every_returned_code_is_in_the_dictionary() {
        let cases = [
            quality_gate(&proc(), &qa_task(), &[], &[], &[], &[]),
            quality_gate(
                &proc(),
                &qa_task(),
                &[],
                &[],
                &[],
                &[execution(ExecutionStatus::Running)],
            ),
        ];
        for g in cases.into_iter().flatten() {
            for d in [g.accept_denied, g.return_denied].into_iter().flatten() {
                assert!(
                    DENIAL_CODES.contains(&d.code.as_str()),
                    "{} is not in DENIAL_CODES",
                    d.code
                );
            }
        }
        assert_eq!(DENIAL_CODES.len(), 3);
    }
}
