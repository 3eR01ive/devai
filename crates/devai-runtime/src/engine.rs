//! Workflow engine: drives a task through its AUTONOMOUS stages by running
//! the assigned role, publishing the contract-shaped output, and committing
//! the transition **through `devai-core`**.
//!
//! Sovereignty is structural here: the engine never edits state directly —
//! it calls the same pure commands the CLI calls, so an agent can only ever
//! do what the workflow already permits. Human gates (APPROVAL, final
//! acceptance in QA) and holds stop the run; the engine does not "pass" them.

use std::path::{Path, PathBuf};

use chrono::Utc;
use devai_core::error::CoreError;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::config::Profile;
use devai_store::{ArtifactDraft, Store, StoreError};

use crate::adapters::{self, AdapterError, AgentRuntime, ExecOutput, ExecRequest};
use crate::evaluation;
use crate::gate;
use crate::prompt::build_user_prompt;
use crate::worktree;

/// Stage the auditor answers in: the agent emits an
/// evaluation-assessment; the PLATFORM assembles the quality-report@4.
const ASSESSMENT_CONTRACT: (&str, u32) = ("evaluation-assessment", 1);
/// @4 extends @3 (QA rule "every criteria entry cites concrete evidence":
/// `evaluation.checks[].evidence` required, `minItems: 1`) with the required
/// embedded `workspace_revision` the platform captured at execution start —
/// a report is now traceable to the exact HEAD + dirty-tree digest + profile
/// name@version + roles slug@version it was written against. The version is
/// pinned HERE (code), so the mechanical gate cannot be bypassed by an old
/// schema text lingering in a home's contract table (seeding is insert-only;
/// @1..@3 stay in the store untouched as history).
const QUALITY_REPORT_CONTRACT: (&str, u32) = ("quality-report", 4);

/// Run-artifact root under the data directory (REQ-WT-05): evidence is
/// platform-declared — `<home>/evidence/<task>/<execution>` — and agents
/// receive it as `DEVAI_EVIDENCE_DIR`; nothing of a run may land in the
/// source tree.
const EVIDENCE_SUBDIR: &str = "evidence";
/// Mechanical-probe transcript persisted per QA execution (REQ-WT-05 AC1).
/// Spelled once; referenced by path, never duplicated as a literal.
const PROBE_EVIDENCE_FILE: &str = "probes.txt";
/// Rendered auditor prompt persisted per QA execution. This is the
/// observation surface REQ-QAD-004 needs: the dogfood test asserts on the
/// ACTUAL text fed to the auditor, and the prompt otherwise exists only
/// transiently inside the adapter (adapters.rs:76). Same platform-evidence
/// location and the same write-fails-the-run discipline as probes.txt
/// (REQ-WT-05) — nothing lands in a checkout.
const AUDITOR_PROMPT_FILE: &str = "auditor_prompt.txt";

/// Render the `APPROVED_DECISIONS` prompt section from the STORED decision
/// rows (REQ-QAD-001/003). Pure formatting of already-persisted fields —
/// no reconstruction from any artifact head, no re-derivation, no re-sorting:
/// `list_decisions` returns `ORDER BY created_at`, and that stored order is
/// reproduced verbatim (OQ-D). Multi-line rationales are printed under a
/// continuation indent so a stored fact stays byte-identical to what the
/// human wrote. An empty table yields one explicit marker line, so the QA
/// prompt keeps a constant section structure across runs (OQ-E) and the
/// auditor can see the ABSENCE of decisions as a fact, not a silent omission.
fn decisions_prompt_text(decisions: &[Decision]) -> String {
    if decisions.is_empty() {
        return "(no decisions recorded for this task)\n".to_string();
    }
    let mut out = String::new();
    for d in decisions {
        out.push_str(&format!(
            "- kind: {}\n  verdict: {}\n  actor: {}\n  created_at: {}\n",
            d.kind,
            d.verdict.as_str(),
            d.actor,
            d.created_at.to_rfc3339(),
        ));
        match d.rationale.as_deref() {
            // OQ-B: a stored rationale is printed as written; a NULL one is
            // marked explicitly rather than silently dropped.
            Some(text) => {
                for (i, line) in text.lines().enumerate() {
                    if i == 0 {
                        out.push_str(&format!("  rationale: {line}\n"));
                    } else {
                        out.push_str(&format!("  {line}\n"));
                    }
                }
            }
            None => out.push_str("  rationale: (none stored)\n"),
        }
    }
    out
}

/// Upper bound of role executions in one `advance` call (QA return7 п.(г):
/// this used to be an inline `16` literal in `Engine::new`). It is a
/// platform run budget — the StepLimit guard stops a misconfigured role
/// chain from spinning forever — named here once and overridable via
/// [`Engine::with_max_steps`].
pub const DEFAULT_MAX_STEPS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("role definition `{0}` not found — run `devai init`")]
    MissingRole(String),
    #[error("no current {0} artifact; the workflow data is inconsistent")]
    MissingArtifact(&'static str),
    #[error("engine reached the {0}-step safety limit while in state {1}")]
    StepLimit(usize, &'static str),
    #[error("no evaluation profile for `quality-auditor` in {0} — run `devai init`")]
    MissingProfile(String),
    #[error("contract {0}@{1} not found — run `devai init`")]
    MissingContract(String, u32),
    /// Worktree isolation failed (creation, reuse, or the platform commit).
    /// The message carries the typed WorktreeError text — which embeds the
    /// REQ it enforces — because a writing stage without its isolation must
    /// stop the run, never fall back to the main checkout.
    #[error(transparent)]
    Worktree(#[from] worktree::WorktreeError),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Where the engine stopped and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Gate {
    /// Awaiting a human `approve`/`reject` in APPROVAL.
    WaitingApproval,
    /// A quality report exists; awaiting a human `accept`/`return` in QA.
    WaitingAccept,
    /// The CURRENT quality report is bound to exactly the workspace revision
    /// recomputed now: the auditor was NOT run. Accept/return the report, or
    /// re-audit explicitly (`--force-reaudit`) — never silently.
    ReportFresh,
    /// Task finished (ACCEPTED / CANCELLED).
    Terminal(TaskState),
    /// Task is parked (WAITING_FOR_INPUT / PAUSED / BLOCKED / FAILED).
    Hold(TaskState),
}

impl std::fmt::Display for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gate::WaitingApproval => f.write_str("human gate: approve or reject the requirements"),
            Gate::WaitingAccept => f.write_str("human gate: accept or return the result"),
            Gate::ReportFresh => {
                f.write_str("report is FRESH for this workspace revision — accept/return it, or re-audit explicitly (--force-reaudit); the auditor does not re-run on unchanged code")
            }
            Gate::Terminal(s) => write!(f, "task is {}", s.as_str()),
            Gate::Hold(s) => write!(f, "task is held in {}", s.as_str()),
        }
    }
}

/// One autonomous stage the engine actually ran.
#[derive(Clone, Debug, serde::Serialize)]
pub struct StepOutcome {
    pub from: TaskState,
    pub to: TaskState,
    pub role: String,
    pub agent: String,
    pub execution_id: Id,
    pub artifact_id: Id,
    pub artifact_kind: ArtifactKind,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RunReport {
    pub task_id: Id,
    pub now: TaskState,
    pub steps: Vec<StepOutcome>,
    pub gate: Gate,
}

pub struct Engine<'a> {
    store: &'a Store,
    max_steps: usize,
    /// Declarative QA rules: the first assessment profile applying to
    /// `quality-auditor`, loaded from the home config tree.
    profile: Option<Profile>,
    /// Directory where the platform executes probes (the project workspace).
    workspace: PathBuf,
    /// Explicit human override of the QA freshness gate: re-audit despite a
    /// fresh CURRENT report. Never set implicitly — the gate only stops.
    force_reaudit: bool,
}

impl<'a> Engine<'a> {
    /// Engine bound to a store; the profile comes from `$DEVAI_CONFIG`
    /// (a config tree rooted at an explicit `devai init --config` dir) or
    /// `<home>/config`, and probes run in the current working directory.
    pub fn new(store: &'a Store) -> Self {
        let cfg_dir = std::env::var_os("DEVAI_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| store.root().join("config"));
        let profiles = devai_store::config::load_profiles(&cfg_dir.join("evaluation")).unwrap_or_default();
        let profile = profiles.into_iter().find(|p| p.applies_to_roles.iter().any(|r| r == "quality-auditor"));
        let workspace = std::env::current_dir().unwrap_or_else(|_| store.root().to_path_buf());
        Engine { store, max_steps: DEFAULT_MAX_STEPS, profile, workspace, force_reaudit: false }
    }

    /// Override the probe workspace (tests, monorepo subdirs).
    pub fn with_workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = workspace.into();
        self
    }

    /// Override the per-run budget of role executions (default
    /// [`DEFAULT_MAX_STEPS`]).
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Provide the profile explicitly instead of loading it from the home.
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Explicit human decision to re-audit an unchanged workspace. The flag
    /// persists an override record (decision + event) — it is never implied.
    pub fn with_force_reaudit(mut self, force: bool) -> Self {
        self.force_reaudit = force;
        self
    }

    /// The workspace revision a NEW execution would be bound to, computed
    /// platform-side right now against the MAIN workspace: git HEAD +
    /// dirty-tree digest, the loaded evaluation profile `name@version`, and
    /// every current role definition `slug@version` from the store.
    pub fn current_revision(&self) -> RuntimeResult<WorkspaceRevision> {
        self.revision_at(&self.workspace)
    }

    /// Revision computed against an explicit directory (same four
    /// components); the engine binds an execution to the revision of the
    /// workspace that execution actually evaluates.
    fn revision_at(&self, workspace: &Path) -> RuntimeResult<WorkspaceRevision> {
        let roles = self.current_roles()?;
        Ok(crate::revision::compute(workspace, self.profile.as_ref(), &roles))
    }

    /// The workspace a stage of THIS task evaluates against: the task's
    /// platform-created worktree once that branch is live with its directory
    /// present (REQ-WT-03 — gates and mechanical probes must measure the code
    /// the writing agent actually changed, not the user's main checkout), the
    /// main checkout before any writing stage ran. The freshness gate
    /// recomputes on the SAME basis an audit bound itself to, so isolation
    /// never silently breaks the @4 comparison.
    fn eval_workspace(&self, task: &Task) -> PathBuf {
        let dir = worktree::path_for(self.store.root(), task.id);
        if worktree::has_branch(&self.workspace, task.id) && dir.is_dir() {
            dir
        } else {
            self.workspace.clone()
        }
    }

    /// Platform-declared run-artifact directory for one execution
    /// (`DEVAI_HOME/evidence/<task>/<execution>`, REQ-WT-05 AC1).
    fn evidence_dir(&self, task_id: Id, execution_id: Id) -> PathBuf {
        self.store.root().join(EVIDENCE_SUBDIR).join(task_id.to_string()).join(execution_id.to_string())
    }

    /// Latest stored version per role slug, as sorted `slug@version` — the
    /// role configuration in force for new executions.
    pub fn current_roles(&self) -> RuntimeResult<Vec<String>> {
        let mut latest: std::collections::BTreeMap<String, u32> = Default::default();
        for r in self.store.list_roles()? {
            let e = latest.entry(r.slug).or_insert(0);
            if r.version > *e {
                *e = r.version;
            }
        }
        Ok(latest.iter().map(|(slug, v)| format!("{slug}@{v}")).collect())
    }

    /// Advance the task through every autonomous stage until a human gate,
    /// hold or terminal state. `agent_name` selects the `AgentInstance`
    /// (builtin fallbacks: `mock`, `claude`).
    pub fn advance(&self, task_ref: &str, agent_name: &str, actor: &str) -> RuntimeResult<RunReport> {
        let mut steps = Vec::new();
        loop {
            let task = self.store.resolve_task(task_ref)?;

            // DRAFT -> ANALYSIS is bookkeeping, not a role execution.
            if task.state == TaskState::Draft {
                let m = workflow::start(&task, actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                continue;
            }

            let (role_slug, kind) = match task.state {
                TaskState::Analysis => ("requirements-analyst", ArtifactKind::RequirementsSpec),
                TaskState::Review => ("requirements-reviewer", ArtifactKind::ReviewReport),
                TaskState::Implementation | TaskState::Rework => ("implementation-agent", ArtifactKind::ImplementationReport),
                TaskState::Qa => ("quality-auditor", ArtifactKind::QualityReport),
                TaskState::Approval => return Ok(self.report(&task, steps, Gate::WaitingApproval)),
                TaskState::Accepted | TaskState::Cancelled => return Ok(self.report(&task, steps, Gate::Terminal(task.state))),
                s => return Ok(self.report(&task, steps, Gate::Hold(s))),
            };

            if steps.len() >= self.max_steps {
                return Err(RuntimeError::StepLimit(self.max_steps, task.state.as_str()));
            }

            // QA freshness gate (REQ-005): BEFORE launching any auditor run,
            // compare the freshly computed workspace revision with the one
            // embedded in the CURRENT quality report. If the workspace is
            // provably unchanged, the report is still valid — STOP and ask
            // for an explicit human decision instead of silently re-running
            // the auditor (the 2026-11-01/02 double-audit dogfood failure).
            // An unbound report (@1..@3, or a non-git workspace) can never be
            // proven fresh: the gate is inert and the audit proceeds.
            if kind == ArtifactKind::QualityReport {
                if let Some((report, revision)) = self.fresh_bound_report(&task)? {
                    if self.force_reaudit {
                        // Explicit human override: honor it, persist the
                        // decision + event so the re-audit is attributable.
                        self.store.record_decision(&Decision {
                            id: new_id(),
                            task_id: task.id,
                            kind: "qa_reaudit_override".into(),
                            verdict: Verdict::Approved,
                            actor: actor.to_string(),
                            rationale: Some(format!("workspace is fresh for quality-report v{}; explicit --force-reaudit", report.seq)),
                            refs: vec![report.id.to_string()],
                            created_at: Utc::now(),
                        })?;
                        self.store.insert_event(
                            Some(task.id),
                            event_kind::QA_REAUDIT_FORCED,
                            actor,
                            serde_json::json!({
                                "current_report": report.id.to_string(),
                                "report_seq": report.seq,
                                "workspace_revision": revision,
                            }),
                        )?;
                    } else {
                        self.store.insert_event(
                            Some(task.id),
                            event_kind::QA_FRESH_AWAITING_DECISION,
                            actor,
                            serde_json::json!({
                                "current_report": report.id.to_string(),
                                "report_seq": report.seq,
                                "workspace_revision": revision,
                                "ask": "accept/return the fresh report, or re-audit explicitly (--force-reaudit)",
                            }),
                        )?;
                        return Ok(self.report(&task, steps, Gate::ReportFresh));
                    }
                }
            }

            let step = self.run_stage(&task, role_slug, kind, agent_name, actor)?;
            let gated = step.artifact_kind == ArtifactKind::QualityReport;
            steps.push(step);
            if gated {
                // The auditor has reported; acceptance belongs to a human.
                let task = self.store.resolve_task(task_ref)?;
                return Ok(self.report(&task, steps, Gate::WaitingAccept));
            }
        }
    }

    fn report(&self, task: &Task, steps: Vec<StepOutcome>, gate: Gate) -> RunReport {
        RunReport { task_id: task.id, now: task.state, steps, gate }
    }

    /// Returns `Some((current_report_artifact, embedded_revision))` when the
    /// CURRENT quality report is bound to a comparable workspace revision
    /// that EQUALS the one recomputed right now — i.e. the report is FRESH
    /// and the auditor must not silently re-run. `None` means "run the audit":
    /// no current report (first QA), an unbound report (@1..@3, whose findings
    /// are bound to no recorded state), a non-comparable revision (no git), or
    /// a genuine mismatch. Freshness requires ALL FOUR components to match
    /// (A-002): a profile or role version bump alone is stale, forcing re-audit.
    fn fresh_bound_report(&self, task: &Task) -> RuntimeResult<Option<(Artifact, WorkspaceRevision)>> {
        let artifacts = self.store.artifacts(task.id)?;
        // Selection by status=CURRENT alone (REQ-005): schema v3 keeps at
        // most one CURRENT per (task_id, kind) — DB-pinned — so the filter
        // is already single-valued; the `max_by_key(seq)` is a defensive
        // belt for legacy-shape slices, not a UI-side workaround.
        let Some(report) = artifacts
            .iter()
            .filter(|a| a.kind == ArtifactKind::QualityReport && a.status == ArtifactStatus::Current)
            .max_by_key(|a| a.seq)
            .cloned()
        else {
            return Ok(None);
        };
        let body = self.store.read_blob(&report.sha256)?;
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return Ok(None);
        };
        let Some(embedded) = value.get("workspace_revision").cloned() else {
            return Ok(None); // unbound (pre-@4) report: freshness cannot be claimed
        };
        let Ok(bound) = serde_json::from_value::<WorkspaceRevision>(embedded) else {
            return Ok(None);
        };
        if !bound.is_comparable() {
            return Ok(None); // no git HEAD/digest: never report fresh
        }
        let fresh = self.revision_at(&self.eval_workspace(task))?;
        if fresh == bound {
            Ok(Some((report, bound)))
        } else {
            Ok(None)
        }
    }

    /// Run exactly one stage end-to-end: guard, execute, publish, commit.
    fn run_stage(&self, task: &Task, role_slug: &str, kind: ArtifactKind, agent_name: &str, actor: &str) -> RuntimeResult<StepOutcome> {
        // Sovereignty check first: the role may only run in this stage.
        workflow::check_execution_allowed(role_slug, task)?;

        let role = self.store.get_role(role_slug)?.ok_or_else(|| RuntimeError::MissingRole(role_slug.to_string()))?;
        let agent = self.resolve_agent(agent_name)?;

        let artifacts = self.store.artifacts(task.id)?;
        // Status-only selection, single-valued since schema v3 (REQ-005);
        // the seq tie-break below is the same defensive belt as in
        // `fresh_bound_report` — never a substitute for the domain invariant.
        let spec = artifacts
            .iter()
            .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current)
            .max_by_key(|a| a.seq)
            .cloned();

        // Stage inputs come from CURRENT artifacts only — stale versions are
        // never fed to a role.
        let mut input_arts: Vec<Artifact> = Vec::new();
        match kind {
            ArtifactKind::RequirementsSpec => {}
            ArtifactKind::ReviewReport | ArtifactKind::ImplementationReport => {
                input_arts.push(spec.as_ref().ok_or(RuntimeError::MissingArtifact("REQUIREMENTS_SPEC"))?.clone());
            }
            ArtifactKind::QualityReport => {
                input_arts.push(spec.as_ref().ok_or(RuntimeError::MissingArtifact("REQUIREMENTS_SPEC"))?.clone());
                if let Some(rep) = artifacts
                    .iter()
                    .filter(|a| a.kind == ArtifactKind::ImplementationReport && a.status == ArtifactStatus::Current)
                    .max_by_key(|a| a.seq)
                {
                    input_arts.push(rep.clone());
                }
            }
            _ => {}
        }
        let mut inputs: Vec<(String, String)> = input_arts
            .iter()
            .map(|a| -> RuntimeResult<(String, String)> { Ok((format!("{} v{}", a.kind.as_str(), a.seq), self.content(a)?)) })
            .collect::<RuntimeResult<_>>()?;

        let is_qa = kind == ArtifactKind::QualityReport;
        // REQ-QAD-001/002: the auditor judging spec-fidelity must see the
        // human's recorded decisions — the approve rationale is where the
        // open questions got ratified (precedent 01a1a0b5-0fcf: a rejected
        // OQ alternative was implemented and scored 1.0 because nothing in
        // the prompt named the human's ruling). Stored rows verbatim via
        // `list_decisions`; EVERY decision of the task is included (OQ-A —
        // the verdict field tells the auditor which was an approval), and
        // the section sits between the implementation report and the
        // mechanical evidence so the section order is fixed and
        // deterministic (REQ-QAD-002).
        if is_qa {
            inputs.push((
                "APPROVED_DECISIONS (all stored human decisions for this task, reproduced verbatim; approve rationales are the ratified resolutions of the spec's open questions)".to_string(),
                decisions_prompt_text(&self.store.list_decisions(task.id)?),
            ));
        }
        // QA stage: the platform runs the profile's probes FIRST, so the
        // auditor sees real mechanical evidence and cannot invent any.
        let eval_run = if is_qa {
            let profile = self.profile.as_ref().ok_or_else(|| {
                let dir = std::env::var_os("DEVAI_CONFIG")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.store.root().join("config"));
                RuntimeError::MissingProfile(dir.join("evaluation").display().to_string())
            })?;
            // REQ-WT-03: probes measure the task's evaluation workspace —
            // the isolation worktree once one is live for this task.
            let run = evaluation::execute(profile, &self.eval_workspace(task));
            inputs.push(("MECHANICAL_EVIDENCE (platform-run probes; rubric scores are yours to confirm)".to_string(), run.evidence_text.clone()));
            Some(run)
        } else {
            None
        };

        // In QA the AGENT answers in assessment shape; the quality-report@4
        // artifact is assembled by the platform afterwards.
        let contract_schema: Option<String> = if is_qa {
            Some(
                self.store
                    .get_contract(ASSESSMENT_CONTRACT.0, ASSESSMENT_CONTRACT.1)?
                    .ok_or_else(|| RuntimeError::MissingContract(ASSESSMENT_CONTRACT.0.into(), ASSESSMENT_CONTRACT.1))?,
            )
        } else {
            // A declared-but-unloadable contract is an error, not a silent
            // "no schema": corruption must not masquerade as absence.
            match role.output_contract.as_ref() {
                Some(c) => self.store.get_contract(&c.name, c.version)?,
                None => None,
            }
        };

        let now = Utc::now();
        // Capture the workspace revision at execution START (A-004): the
        // recorded value freezes what the result is about; edits DURING the
        // run do not rewrite history but will make a later recomputation
        // differ — marking the report stale-after-the-fact, exactly as they
        // should. Observation only: nothing here can fail or mutate the run.
        // The computation targets the task's EVALUATION workspace: main
        // checkout before any worktree exists, the worktree once one does
        // (the row then binds exactly what the stage measures, REQ-WT-03).
        let revision = self.revision_at(&self.eval_workspace(task))?;
        let execution = RoleExecution {
            id: new_id(),
            task_id: task.id,
            role_slug: role.slug.clone(),
            role_version: role.version,
            agent_name: Some(agent.name.clone()),
            status: ExecutionStatus::Running,
            inputs: input_arts.iter().map(|a| a.id.to_string()).collect(),
            summary: None,
            error: None,
            started_at: now,
            finished_at: None,
            workspace_revision: Some(revision.clone()),
        };
        self.store.start_execution(&execution)?;

        // REQ-WT-01: a writing role runs inside platform isolation. The
        // worktree is created (or the task's branch re-attached) BEFORE the
        // agent executes, from the git_head component of the revision bound
        // to this row — AC1: the branch `rework/<task-id>` points at exactly
        // that commit. Writing membership is the role's DECLARATIVE `tools:`
        // capability (`role.writes_workspace()`), never an artifact-kind
        // guess. A task bound to no revision commit fails the execution
        // explicitly — there is no silent fallback to the main checkout's
        // current state (AC2).
        let writing = role.writes_workspace();
        let evidence = self.evidence_dir(task.id, execution.id);
        let mut isolated: Option<worktree::Worktree> = None;
        if writing {
            match worktree::prepare(&self.workspace, self.store.root(), task.id, revision.git_head.as_deref()) {
                Ok(wt) => {
                    if let Err(e) = std::fs::create_dir_all(&evidence) {
                        let _ = self.store.finish_execution(
                            &execution,
                            ExecutionStatus::Failed,
                            None,
                            Some(&format!("cannot create platform evidence dir {}: {e}", evidence.display())),
                            Utc::now(),
                        );
                        return Err(RuntimeError::Store(e.into()));
                    }
                    self.store.insert_event(
                        Some(task.id),
                        event_kind::WORKTREE_PREPARED,
                        &execution.role_slug,
                        serde_json::json!({
                            "execution": execution.id.to_string(),
                            "branch": wt.branch,
                            "worktree": wt.dir.display().to_string(),
                            "bound_head": revision.git_head,
                        }),
                    )?;
                    isolated = Some(wt);
                }
                Err(e) => {
                    let _ = self.store.finish_execution(&execution, ExecutionStatus::Failed, None, Some(&e.to_string()), Utc::now());
                    return Err(e.into());
                }
            }
        }

        // REQ-WT-05 AC1: the probe transcript is a run artifact — stored
        // under the platform-declared evidence root, referenced from the
        // prompt by path; never written into any checkout.
        if let Some(run) = &eval_run {
            if let Err(e) = std::fs::create_dir_all(&evidence)
                .and_then(|_| std::fs::write(evidence.join(PROBE_EVIDENCE_FILE), &run.evidence_text))
            {
                let _ = self.store.finish_execution(&execution, ExecutionStatus::Failed, None, Some(&format!("cannot store probe evidence in {}: {e}", evidence.display())), Utc::now());
                return Err(RuntimeError::Store(e.into()));
            }
            inputs.push((
                "MECHANICAL_EVIDENCE_FILE (platform-run probes, persisted copy)".to_string(),
                evidence.join(PROBE_EVIDENCE_FILE).display().to_string(),
            ));
        }

        // Ground truth for the writing agent: it must know its process
        // directory is the worktree and its artifact sink is the evidence
        // dir, not the user's checkout (the 2026-11-01 dogfood wrote an
        // evidence file into the source tree because nothing declared it).
        if let Some(wt) = &isolated {
            inputs.push((
                "PLATFORM WORKTREE FACTS".to_string(),
                format!(
                    "Your working directory is the platform-created git worktree `{}` on branch `{}` (created from bound commit `{}`). You do NOT work in the user's main checkout. The platform owns the branch and the commit: do not create branches, do not commit, do not run any git write command — read-only git only. Run artifacts (logs, evidence files) go ONLY into `{}` (environment DEVAI_EVIDENCE_DIR), never into the checkout. The published report's `branch`, `worktree` and `commit` fields are stamped by the platform with these ground-truth values.",
                    wt.dir.display(),
                    wt.branch,
                    revision.git_head.as_deref().unwrap_or("(unbound)"),
                    evidence.display(),
                ),
            ));
        }

        // The auditor ANSWERS in assessment shape; quality-report@4 describes
        // the artifact the platform assembles afterwards. The prompt must not
        // ask for a shape the agent cannot honestly produce.
        let mut exec_role = role.clone();
        if is_qa {
            exec_role.output_contract = Some(ContractRef {
                name: ASSESSMENT_CONTRACT.0.into(),
                version: ASSESSMENT_CONTRACT.1,
            });
        }
        // Observation surface for the QA prompt (REQ-QAD-004, resolves OQ-F):
        // the rendered auditor prompt is persisted next to the probe
        // transcript, so a dogfood test — and a human debugging a
        // spec-fidelity score — can assert on the EXACT text the auditor is
        // fed. The render uses the same pure `build_user_prompt` with the
        // same arguments `adapters::claude` will pass for this request
        // (title, business, ordered inputs, the assessment contract, its
        // schema). A write failure fails the execution honestly, mirroring
        // the probes.txt discipline above (REQ-WT-05); the QA branch has
        // already created this directory.
        if is_qa {
            let prompt = build_user_prompt(
                &task.title,
                &task.business_description,
                &inputs,
                Some((ASSESSMENT_CONTRACT.0, ASSESSMENT_CONTRACT.1)),
                contract_schema.as_deref(),
            );
            if let Err(e) = std::fs::write(evidence.join(AUDITOR_PROMPT_FILE), prompt) {
                let _ = self.store.finish_execution(&execution, ExecutionStatus::Failed, None, Some(&format!("cannot store auditor prompt in {}: {e}", evidence.display())), Utc::now());
                return Err(RuntimeError::Store(e.into()));
            }
        }

        let req = ExecRequest {
            task_id: task.id,
            title: task.title.clone(),
            business: task.business_description.clone(),
            role: exec_role,
            provider: agent.provider.clone(),
            model: agent.model.clone(),
            inputs: inputs.clone(),
            contract_schema: contract_schema.clone(),
            // REQ-WT-02 AC1: a writing role is NEVER handed the main
            // checkout path — its workspace is the isolation worktree.
            workspace: Some(isolated.as_ref().map(|w| w.dir.clone()).unwrap_or_else(|| self.workspace.clone())),
            evidence: if writing { Some(evidence) } else { None },
        };

        let out = match AgentRuntime::run(&req) {
            Ok(o) => o,
            Err(e) => {
                // The execution is recorded as failed; the task state is
                // untouched — history says what happened, workflow says nothing
                // moved.
                let _ = self.store.finish_execution(&execution, ExecutionStatus::Failed, None, Some(&e.to_string()), Utc::now());
                return Err(e.into());
            }
        };

        // QA: merge the platform's probe results with the auditor's
        // assessment into the machine quality-report@4 and run the
        // deterministic verdict function. Assembly happens BEFORE the
        // execution is closed, so a broken assembly is honestly FAILED.
        // The SAME revision recorded on the execution row is embedded in the
        // artifact — no recomputation drift between row and report.
        let (content, exec_summary, report_contract) = if let Some(wt) = &isolated {
            // REQ-WT-06/04: the platform captures the agent's work in a
            // commit on the task's OWN branch, stamps the report with the
            // resulting ground truth, and re-validates the stamped body —
            // a published implementation report cannot name a directory
            // that holds no committed state. Failure here is an honestly
            // FAILED execution, never an unstamped artifact.
            match self.capture_writing_result(task, &execution, wt, contract_schema.as_deref(), &out) {
                Ok((content, summary)) => (content, Some(summary), role.output_contract.clone()),
                Err(e) => {
                    let _ = self.store.finish_execution(&execution, ExecutionStatus::Failed, None, Some(&e.to_string()), Utc::now());
                    return Err(e);
                }
            }
        } else {
            match (&eval_run, is_qa) {
            (Some(run), true) => {
                let profile = self.profile.as_ref().expect("eval_run implies profile present");
                let assessment: serde_json::Value = serde_json::from_slice(&out.content)
                    .map_err(|e| AdapterError::BadOutput(format!("assessment reply is not valid JSON: {e}")))?;
                let assembled = evaluation::assemble(profile, run, &assessment, &execution.id.to_string(), &revision);
                let text = serde_json::to_string_pretty(&assembled.json)
                    .map_err(|e| AdapterError::Agent(format!("cannot serialize assembled report: {e}")))?;
                let schema = self
                    .store
                    .get_contract(QUALITY_REPORT_CONTRACT.0, QUALITY_REPORT_CONTRACT.1)?
                    .ok_or_else(|| RuntimeError::MissingContract(QUALITY_REPORT_CONTRACT.0.into(), QUALITY_REPORT_CONTRACT.1))?;
                if let Err(e) = adapters::validate_against_contract(&text, Some(&schema)) {
                    let _ = self.store.finish_execution(
                        &execution,
                        ExecutionStatus::Failed,
                        None,
                        Some(&format!(
                            "platform-assembled report violates {}@{}: {e}",
                            QUALITY_REPORT_CONTRACT.0, QUALITY_REPORT_CONTRACT.1
                        )),
                        Utc::now(),
                    );
                    return Err(e.into());
                }
                (
                    text.into_bytes(),
                    Some(format!(
                        "{} (evaluation {}, S={:.3})",
                        assembled.recommendation, assembled.evaluation_verdict, assembled.score
                    )),
                    Some(ContractRef { name: QUALITY_REPORT_CONTRACT.0.into(), version: QUALITY_REPORT_CONTRACT.1 }),
                )
            }
            _ => (out.content.clone(), out.summary.clone(), role.output_contract.clone()),
            }
        };
        self.store.finish_execution(&execution, ExecutionStatus::Completed, exec_summary.as_deref(), None, Utc::now())?;

        // Publish the result as a formal, content-addressed artifact.
        let depends: Vec<Id> = spec.iter().map(|s| s.id).collect();
        let name = report_contract
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| kind.as_str().to_lowercase());
        let artifact = self.store.publish_artifact(ArtifactDraft {
            task_id: task.id,
            kind,
            name,
            category: DataCategory::Artifact,
            content: content.clone(),
            media_type: "application/json".into(),
            contract: report_contract,
            created_by: execution.id.to_string(),
            execution_id: Some(execution.id),
            depends_on: depends,
        })?;

        // Commit the workflow effect through the core commands — the engine
        // cannot bypass the state machine.
        let fresh = self.store.artifacts(task.id)?;
        let json: serde_json::Value = serde_json::from_slice(&content).unwrap_or(serde_json::Value::Null);
        let m = match kind {
            ArtifactKind::RequirementsSpec => {
                workflow::requirements_published(task, &fresh, Some(&format!("{role_slug} execution {}", execution.id)), actor, Utc::now())?
            }
            ArtifactKind::ReviewReport => {
                let verdict = json
                    .get("verdict")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Verdict::parse(s).ok())
                    .unwrap_or(Verdict::NeedsRevision);
                workflow::review_recorded(task, verdict, out.summary.as_deref(), actor, Utc::now())?
            }
            ArtifactKind::ImplementationReport => {
                // REQ-RG-01/02: the empty-rework gate serializes INSIDE the
                // core command — the engine only hands it the stored facts
                // (stamped commits of the report bodies, the task's human
                // ratifications, the QA-return rationale). A REWORK
                // publication naming the previous report's identical commit
                // parks the task in WAITING_FOR_INPUT on a blocking question
                // instead of riding silently into QA; the engine creates no
                // ratification of its own (REQ-RG-03 AC2).
                let facts = gate::collect_implementation_report_facts(self.store, task.id, &artifact, &fresh, out.summary.clone())?;
                workflow::implementation_reported(task, &facts, actor, Utc::now())?
            }
            ArtifactKind::QualityReport => {
                // QA produced its report; acceptance is a HUMAN decision.
                // No state transition: stop right here.
                return Ok(StepOutcome {
                    from: task.state,
                    to: task.state,
                    role: role_slug.to_string(),
                    agent: agent.name.clone(),
                    execution_id: execution.id,
                    artifact_id: artifact.id,
                    artifact_kind: kind,
                    summary: exec_summary,
                });
            }
            other => {
                return Err(CoreError::guard(format!("engine produced an unexpected output kind {other:?}")).into());
            }
        };
        self.store.commit(task.lock_version, &m)?;

        Ok(StepOutcome {
            from: task.state,
            to: m.task.state,
            role: role_slug.to_string(),
            agent: agent.name.clone(),
            execution_id: execution.id,
            artifact_id: artifact.id,
            artifact_kind: kind,
            summary: exec_summary,
        })
    }

    /// Commit + stamp for one completed writing execution (REQ-WT-06,
    /// REQ-WT-04). The commit touches nothing but `refs/heads/rework/<task>`
    /// — the platform's only ref channel (REQ-WT-08 (ii)) — because every
    /// git command here runs with the WORKTREE as its directory; the main
    /// checkout's HEAD, index and working tree are not addressed at all.
    /// Then the report body is stamped with ground truth — the agent's own
    /// claims about branch/worktree/commit are overwritten, and the stamped
    /// body must pass the role's contract again, so the published artifact
    /// provably names resolvable repository state (the 2026-11-01 "worked
    /// uncommitted on main" defect becomes structurally unreportable).
    fn capture_writing_result(
        &self,
        task: &Task,
        execution: &RoleExecution,
        wt: &worktree::Worktree,
        contract_schema: Option<&str>,
        out: &ExecOutput,
    ) -> Result<(Vec<u8>, String), RuntimeError> {
        let one_line: String = task.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let sha = worktree::commit_platform_work(&wt.dir, &format!("{}: {one_line}", worktree::COMMIT_SUBJECT_PREFIX), task.id, execution.id)?;
        let mut body: serde_json::Value = serde_json::from_slice(&out.content)
            .map_err(|e| AdapterError::BadOutput(format!("implementation report is not valid JSON, cannot stamp: {e}")))?;
        let obj = body
            .as_object_mut()
            .ok_or_else(|| AdapterError::BadOutput("implementation report must be one JSON object — nothing to stamp".to_string()))?;
        obj.insert("branch".to_string(), serde_json::json!(wt.branch));
        obj.insert("worktree".to_string(), serde_json::json!(wt.dir.display().to_string()));
        obj.insert("commit".to_string(), serde_json::json!(sha));
        let text = serde_json::to_string_pretty(&body)
            .map_err(|e| AdapterError::BadOutput(format!("cannot serialize stamped report: {e}")))?;
        // Re-validate the STAMPED body against the role's contract (the
        // mechanical half of implementation-report@2: the guarantee is
        // engine-side, not a promise to the model).
        adapters::validate_against_contract(&text, contract_schema)?;
        self.store.insert_event(
            Some(task.id),
            event_kind::WORKTREE_COMMITTED,
            &execution.role_slug,
            serde_json::json!({
                "execution": execution.id.to_string(),
                "branch": wt.branch,
                "worktree": wt.dir.display().to_string(),
                "commit": sha,
            }),
        )?;
        Ok((text.into_bytes(), format!("branch {} @ {}", wt.branch, sha.chars().take(12).collect::<String>())))
    }

    fn content(&self, a: &Artifact) -> RuntimeResult<String> {
        Ok(String::from_utf8_lossy(&self.store.read_blob(&a.sha256)?).to_string())
    }

    fn resolve_agent(&self, name: &str) -> RuntimeResult<AgentInstance> {
        if let Some(a) = self.store.list_agents()?.into_iter().find(|a| a.name == name) {
            return Ok(a);
        }
        // Builtin fallbacks so `devai task run --agent mock` works in a
        // freshly initialised home without registration.
        if name == "mock" || name == "claude" {
            return Ok(AgentInstance {
                id: new_id(),
                name: name.to_string(),
                provider: name.to_string(),
                model: String::new(),
                runtime: "builtin".into(),
                created_at: Utc::now(),
            });
        }
        Err(devai_store::StoreError::NotFound("agent".into(), name.to_string()).into())
    }
}

#[cfg(test)]
mod decisions_render_tests {
    use super::*;
    use devai_core::types::{new_id, Decision, Verdict};

    fn decision(kind: &str, verdict: Verdict, rationale: Option<&str>) -> Decision {
        Decision {
            id: new_id(),
            task_id: new_id(),
            kind: kind.into(),
            verdict,
            actor: "human".into(),
            rationale: rationale.map(|s| s.to_string()),
            refs: vec![],
            created_at: Utc::now(),
        }
    }

    #[test]
    fn empty_decisions_yield_constant_marker() {
        assert_eq!(decisions_prompt_text(&[]), "(no decisions recorded for this task)\n");
    }

    #[test]
    fn stored_fields_are_reproduced_verbatim_in_stored_order() {
        let a = decision("requirements_approval", Verdict::Approved, Some("OQ1=X"));
        let b = decision("rework", Verdict::NeedsRevision, Some("line1\nline2"));
        let text = decisions_prompt_text(&[a.clone(), b.clone()]);
        assert!(text.contains(&format!("- kind: {}\n  verdict: APPROVED\n  actor: {}\n", a.kind, a.actor)));
        assert!(text.contains("  rationale: OQ1=X\n"));
        assert!(text.contains(&format!("  verdict: {}\n", b.verdict.as_str())));
        assert!(text.contains("  rationale: line1\n  line2\n"));
        assert!(text.find(&a.kind).unwrap() < text.find(&b.kind).unwrap());
    }

    #[test]
    fn null_rationale_is_marked_not_dropped() {
        let d = decision("acceptance", Verdict::Approved, None);
        let text = decisions_prompt_text(&[d]);
        assert!(text.contains("  rationale: (none stored)\n"), "{text}");
    }

    #[test]
    fn render_is_deterministic_for_identical_inputs() {
        let d = decision("requirements_approval", Verdict::Approved, Some("OQ1=X"));
        let x = decisions_prompt_text(&[d.clone()]);
        let y = decisions_prompt_text(&[d]);
        assert_eq!(x, y, "same stored rows -> byte-identical section text");
    }
}
