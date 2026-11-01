//! Workflow engine: drives a task through its AUTONOMOUS stages by running
//! the assigned role, publishing the contract-shaped output, and committing
//! the transition **through `devai-core`**.
//!
//! Sovereignty is structural here: the engine never edits state directly —
//! it calls the same pure commands the CLI calls, so an agent can only ever
//! do what the workflow already permits. Human gates (APPROVAL, final
//! acceptance in QA) and holds stop the run; the engine does not "pass" them.

use chrono::Utc;
use devai_core::error::CoreError;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::{ArtifactDraft, Store, StoreError};

use crate::adapters::{AdapterError, AgentRuntime, ExecRequest};

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
}

impl<'a> Engine<'a> {
    pub fn new(store: &'a Store) -> Self {
        Engine { store, max_steps: 16 }
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
        let inputs: Vec<(String, String)> = input_arts
            .iter()
            .map(|a| -> RuntimeResult<(String, String)> { Ok((format!("{} v{}", a.kind.as_str(), a.seq), self.content(a)?)) })
            .collect::<RuntimeResult<_>>()?;

        let contract_schema = role
            .output_contract
            .as_ref()
            .map(|c| self.store.get_contract(&c.name, c.version).ok().flatten())
            .flatten();

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

        let req = ExecRequest {
            task_id: task.id,
            title: task.title.clone(),
            business: task.business_description.clone(),
            role: role.clone(),
            provider: agent.provider.clone(),
            model: agent.model.clone(),
            inputs: inputs.clone(),
            contract_schema: contract_schema.clone(),
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
        self.store.finish_execution(&execution, ExecutionStatus::Completed, out.summary.as_deref(), None, Utc::now())?;

        // Publish the result as a formal, content-addressed artifact.
        let depends: Vec<Id> = spec.iter().map(|s| s.id).collect();
        let name = role.output_contract.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| kind.as_str().to_lowercase());
        let artifact = self.store.publish_artifact(ArtifactDraft {
            task_id: task.id,
            kind,
            name,
            category: DataCategory::Artifact,
            content: out.content.clone(),
            media_type: "application/json".into(),
            contract: role.output_contract.clone(),
            created_by: execution.id.to_string(),
            execution_id: Some(execution.id),
            depends_on: depends,
        })?;

        // Commit the workflow effect through the core commands — the engine
        // cannot bypass the state machine.
        let fresh = self.store.artifacts(task.id)?;
        let json: serde_json::Value = serde_json::from_slice(&out.content).unwrap_or(serde_json::Value::Null);
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
                    summary: out.summary,
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
            summary: out.summary,
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
