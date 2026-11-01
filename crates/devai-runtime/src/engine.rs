//! Workflow engine: drives a task through its AUTONOMOUS stages by running
//! the assigned role, publishing the contract-shaped output, and committing
//! the transition **through `devai-core`**.
//!
//! Sovereignty is structural here: the engine never edits state directly —
//! it calls the same pure commands the CLI calls, so an agent can only ever
//! do what the workflow already permits. Human gates (APPROVAL, final
//! acceptance in QA) and holds stop the run; the engine does not "pass" them.

use std::path::PathBuf;

use chrono::Utc;
use devai_core::error::CoreError;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::config::Profile;
use devai_store::{ArtifactDraft, Store, StoreError};

use crate::adapters::{self, AdapterError, AgentRuntime, ExecRequest};
use crate::evaluation;

/// Stage the auditor answers in: the agent emits an
/// evaluation-assessment; the PLATFORM assembles the quality-report@2.
const ASSESSMENT_CONTRACT: (&str, u32) = ("evaluation-assessment", 1);
const QUALITY_REPORT_CONTRACT: (&str, u32) = ("quality-report", 2);

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
        Engine { store, max_steps: 16, profile, workspace }
    }

    /// Override the probe workspace (tests, monorepo subdirs).
    pub fn with_workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = workspace.into();
        self
    }

    /// Provide the profile explicitly instead of loading it from the home.
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = Some(profile);
        self
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

    /// Run exactly one stage end-to-end: guard, execute, publish, commit.
    fn run_stage(&self, task: &Task, role_slug: &str, kind: ArtifactKind, agent_name: &str, actor: &str) -> RuntimeResult<StepOutcome> {
        // Sovereignty check first: the role may only run in this stage.
        workflow::check_execution_allowed(role_slug, task)?;

        let role = self.store.get_role(role_slug)?.ok_or_else(|| RuntimeError::MissingRole(role_slug.to_string()))?;
        let agent = self.resolve_agent(agent_name)?;

        let artifacts = self.store.artifacts(task.id)?;
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
        // QA stage: the platform runs the profile's probes FIRST, so the
        // auditor sees real mechanical evidence and cannot invent any.
        let eval_run = if is_qa {
            let profile = self.profile.as_ref().ok_or_else(|| {
                let dir = std::env::var_os("DEVAI_CONFIG")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.store.root().join("config"));
                RuntimeError::MissingProfile(dir.join("evaluation").display().to_string())
            })?;
            let run = evaluation::execute(profile, &self.workspace);
            inputs.push(("MECHANICAL_EVIDENCE (platform-run probes; rubric scores are yours to confirm)".to_string(), run.evidence_text.clone()));
            Some(run)
        } else {
            None
        };

        // In QA the AGENT answers in assessment shape; the quality-report@2
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
        };
        self.store.start_execution(&execution)?;

        // The auditor ANSWERS in assessment shape; quality-report@2 describes
        // the artifact the platform assembles afterwards. The prompt must not
        // ask for a shape the agent cannot honestly produce.
        let mut exec_role = role.clone();
        if is_qa {
            exec_role.output_contract = Some(ContractRef {
                name: ASSESSMENT_CONTRACT.0.into(),
                version: ASSESSMENT_CONTRACT.1,
            });
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
            workspace: Some(self.workspace.clone()),
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
        // assessment into the machine quality-report@2 and run the
        // deterministic verdict function. Assembly happens BEFORE the
        // execution is closed, so a broken assembly is honestly FAILED.
        let (content, exec_summary, report_contract) = match (&eval_run, is_qa) {
            (Some(run), true) => {
                let profile = self.profile.as_ref().expect("eval_run implies profile present");
                let assessment: serde_json::Value = serde_json::from_slice(&out.content)
                    .map_err(|e| AdapterError::BadOutput(format!("assessment reply is not valid JSON: {e}")))?;
                let assembled = evaluation::assemble(profile, run, &assessment, &execution.id.to_string());
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
                        Some(&format!("platform-assembled report violates quality-report@2: {e}")),
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
            ArtifactKind::ImplementationReport => workflow::implementation_reported(task, out.summary.as_deref(), actor, Utc::now())?,
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
