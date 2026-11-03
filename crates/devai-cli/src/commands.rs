use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_runtime::{worktree, Engine, Gate};
use devai_store::{ArtifactDraft, Store, guess_media_type};
use uuid::Uuid;

use crate::outln;
use crate::{ArtifactCmd, AgentCmd, ContractCmd, ExecCmd, ProjectCmd, QuestionCmd, RoleCmd, TaskCmd, Cmd};
use devai_store::seed;

pub struct Ctx {
    pub home: PathBuf,
    pub actor: String,
    pub store: Store,
}

impl Ctx {
    pub fn new(home: Option<&Path>, actor: Option<String>) -> Result<Ctx> {
        let dir = match home {
            Some(p) => p.to_path_buf(),
            None => {
                let base = dirs::home_dir().context("no home directory; pass --home or set DEVAI_HOME")?;
                base.join(".devai")
            }
        };
        let actor = actor.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "user".into()));
        let store = Store::open(&dir).context("open local store")?;
        Ok(Ctx { home: dir, actor, store })
    }

    fn now(&self) -> Timestamp {
        Utc::now()
    }
}

pub fn run(ctx: &Ctx, cmd: Cmd, json: bool) -> Result<()> {
    match cmd {
        Cmd::Init { config } => init(ctx, config.as_deref(), json),
        Cmd::Project(c) => project(ctx, c, json),
        Cmd::Task(c) => task(ctx, c, json),
        Cmd::Artifact(c) => artifact(ctx, c, json),
        Cmd::Question(c) => question(ctx, c, json),
        Cmd::Role(c) => role(ctx, c, json),
        Cmd::Agent(c) => agent(ctx, c, json),
        Cmd::Contract(c) => contract(ctx, c, json),
        Cmd::Exec(c) => exec(ctx, c, json),
        Cmd::Export { task, out } => export(ctx, &task, &out, json),
        Cmd::Import { dir } => import(ctx, &dir, json),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn init(ctx: &Ctx, config_dir: Option<&Path>, json: bool) -> Result<()> {
    let (seeded, cfg_path) = match config_dir {
        Some(dir) => (seed::seed_from(&ctx.store, dir, ctx.now())?, dir.to_path_buf()),
        None => {
            let dir = ctx.home.join("config");
            (seed::seed_defaults(&ctx.store, ctx.now())?, dir)
        }
    };
    if json {
        outln!("{}", serde_json::json!({"home": ctx.home, "config": cfg_path, "roles_seeded": seeded}));
    } else {
        outln!("devai home: {}", ctx.home.display());
        outln!("config tree: {}", cfg_path.display());
        if seeded > 0 {
            outln!("seeded/updated {seeded} role version(s) from config");
            outln!(
                "edit {}/criteria/*.yaml, evaluation/*.yaml or roles/*.yaml and re-run `devai init` to bump versions",
                cfg_path.display()
            );
            if config_dir.is_some() {
                outln!("the engine reads evaluation profiles from $DEVAI_CONFIG — export DEVAI_CONFIG={}", cfg_path.display());
            }
        } else {
            outln!("store already initialized (config unchanged)");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

fn project(ctx: &Ctx, cmd: ProjectCmd, json: bool) -> Result<()> {
    match cmd {
        ProjectCmd::Add { name, description, repo } => {
            let p = Project::new(name, description, repo, ctx.now());
            ctx.store.create_project(&p)?;
            outln!("project {} ({})", p.name, short(&p.id));
        }
        ProjectCmd::List => {
            let ps = ctx.store.list_projects()?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&ps)?);
            } else if ps.is_empty() {
                outln!("(no projects)");
            } else {
                for p in ps {
                    outln!("{}  {}  repos={}", short(&p.id), p.name, p.repos.len());
                }
            }
        }
        ProjectCmd::Get { id } => match ctx.store.get_project(&id)? {
            Some(p) => {
                if json {
                    outln!("{}", serde_json::to_string_pretty(&p)?);
                } else {
                    outln!("id:          {}", p.id);
                    outln!("name:        {}", p.name);
                    outln!("description: {}", p.description);
                    for r in p.repos {
                        outln!("repo:        {r}");
                    }
                }
            }
            None => bail!("project `{id}` not found"),
        },
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// task
// ---------------------------------------------------------------------------

fn task(ctx: &Ctx, cmd: TaskCmd, json: bool) -> Result<()> {
    match cmd {
        TaskCmd::New { title, project, description, description_file } => {
            let project_id = match project.as_deref() {
                Some(name) => Some(ctx.store.get_project(name)?.context(format!("project `{name}` not found"))?.id),
                None => None,
            };
            let desc = match description_file {
                Some(f) => std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?,
                None => description,
            };
            let t = Task::new(title, desc, project_id, ctx.now());
            ctx.store.create_task(&t)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                outln!("task {} created in DRAFT — workflow is owned by devai", short(&t.id));
                outln!("start it with: devai task start {}", short(&t.id));
            }
        }
        TaskCmd::List => {
            let ts = ctx.store.list_tasks()?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&ts)?);
            } else if ts.is_empty() {
                outln!("(no tasks)");
            } else {
                for t in ts {
                    outln!("{}  {:<17}  v{:>2}  {}", short(&t.id), t.state.as_str(), t.requirements_seq, t.title);
                }
            }
        }
        TaskCmd::Get { id } => {
            let t = ctx.store.resolve_task(&id)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                print_task(&t);
            }
        }
        TaskCmd::Status { id } => status(ctx, &id, json)?,
        TaskCmd::Progress { id } => progress(ctx, &id, json)?,
        TaskCmd::Report { id } => show_report(ctx, &id, json)?,
        TaskCmd::Start { id } => op(ctx, &id, |t, _a| workflow::start(t, &ctx.actor, ctx.now()))?,
        TaskCmd::Run { id, agent, workspace, force_reaudit } => task_run(ctx, &id, &agent, workspace.as_deref(), force_reaudit, json)?,
        TaskCmd::Revise { id, file, reason } => revise(ctx, &id, &file, reason.as_deref())?,
        TaskCmd::Review { id, verdict, summary } => {
            let v = parse_verdict(&verdict)?;
            op(ctx, &id, move |t, _a| workflow::review_recorded(t, v, summary.as_deref(), &ctx.actor, ctx.now()))?;
        }
        TaskCmd::Approve { id, rationale } => {
            // The approve act is persisted like every other human-gate act
            // (`rework_merge` below, `qa_reaudit_override` in the engine):
            // a workflow event via `op` AND a stored decision row. The row
            // is what makes the rationale a fact the QA auditor is fed —
            // REQ-QAD-001 read the `decisions` table, and before this the
            // approve rationale existed only inside the event payload, so
            // spec-fidelity scoring never saw the ratified open-question
            // resolutions (precedent 01a1a0b5-0fcf).
            let task = ctx.store.resolve_task(&id)?;
            let spec_ref = ctx
                .store
                .artifacts(task.id)?
                .iter()
                .find(|x| x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current)
                .map(|x| x.id.to_string())
                .unwrap_or_default();
            let decision_rationale = rationale.clone();
            op(ctx, &id, move |t, a| {
                if a.iter().any(|x| x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current) {
                    workflow::approve_requirements(t, rationale.as_deref(), &ctx.actor, ctx.now())
                } else {
                    Err(devai_core::CoreError::guard("no CURRENT REQUIREMENTS_SPEC artifact to approve"))
                }
            })?;
            ctx.store.record_decision(&Decision {
                id: new_id(),
                task_id: task.id,
                kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
                verdict: Verdict::Approved,
                actor: ctx.actor.clone(),
                rationale: decision_rationale,
                refs: if spec_ref.is_empty() { Vec::new() } else { vec![spec_ref] },
                created_at: ctx.now(),
            })?;
        }
        TaskCmd::Reject { id, rationale } => op(ctx, &id, move |t, _a| workflow::reject_requirements(t, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Accept { id, rationale } => op(ctx, &id, move |t, a| workflow::accept(t, a, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Return { id, rationale } => op(ctx, &id, move |t, _a| workflow::return_to_rework(t, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::RatifyEmptyRework { id, rationale } => ratify_empty_rework(ctx, &id, rationale.as_deref(), json)?,
        TaskCmd::Merge { id, workspace, rationale } => task_merge(ctx, &id, workspace.as_deref(), rationale.as_deref(), json)?,
        TaskCmd::Pause { id, reason } => op(ctx, &id, move |t, _a| workflow::pause(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Resume { id } => op(ctx, &id, |t, _a| workflow::resume(t, &ctx.actor, ctx.now()))?,
        TaskCmd::Block { id, reason } => op(ctx, &id, move |t, _a| workflow::block(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Cancel { id, reason } => op(ctx, &id, move |t, _a| workflow::cancel(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Fail { id, reason } => op(ctx, &id, move |t, _a| workflow::fail(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Events { id, limit } => {
            let t = ctx.store.resolve_task(&id)?;
            let evs = ctx.store.events(t.id, limit)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&evs)?);
            } else {
                for e in evs {
                    outln!("{}  {:<28} {:<12} {}", e.created_at.format("%Y-%m-%d %H:%M:%S"), e.kind, e.actor, compact(&e.payload));
                }
            }
        }
    }
    Ok(())
}

/// Load task, run a core command, commit with optimistic locking, report.
fn op<F>(ctx: &Ctx, id: &str, f: F) -> Result<()>
where
    F: FnOnce(&Task, &[Artifact]) -> devai_core::CoreResult<Mutation>,
{
    let task = ctx.store.resolve_task(id)?;
    let artifacts = ctx.store.artifacts(task.id)?;
    let m = f(&task, &artifacts).map_err(anyhow::Error::new)?;
    let staled = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Stale).count();
    let superseded = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Superseded).count();
    ctx.store.commit(task.lock_version, &m)?;
    if m.task.state != task.state {
        outln!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
    } else {
        outln!("recorded ({})", m.events[0].kind);
    }
    if staled > 0 {
        outln!("marked STALE: {staled} dependent artifact(s) — history preserved");
    }
    if superseded > 0 {
        outln!("superseded: {superseded}");
    }
    let ops = workflow::allowed_ops(&m.task);
    if !ops.is_empty() {
        outln!("next: {}", ops.join(" | "));
    }
    Ok(())
}

/// Drive the task through every AUTONOMOUS stage with a real role
/// execution. Stops at human gates (APPROVAL, acceptance) — never past them.
fn task_run(ctx: &Ctx, id: &str, agent: &str, workspace: Option<&std::path::Path>, force_reaudit: bool, json: bool) -> Result<()> {
    let mut engine = Engine::new(&ctx.store).with_force_reaudit(force_reaudit);
    if let Some(w) = workspace {
        engine = engine.with_workspace(w);
    }
    let report = engine.advance(id, agent, &ctx.actor).map_err(anyhow::Error::new)?;
    if json {
        outln!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for s in &report.steps {
        outln!(
            "{} → {}: `{}` on `{}`  artifact {} ({})",
            s.from.as_str(),
            s.to.as_str(),
            s.role,
            s.agent,
            short(&s.artifact_id),
            s.artifact_kind.as_str(),
        );
        if let Some(sum) = &s.summary {
            outln!("   ↳ {}", ellipsis(sum, 160));
        }
    }
    match report.gate {
        Gate::WaitingApproval => outln!("⏸ APPROVAL is a human gate — approve or reject the requirements"),
        Gate::WaitingAccept => {
            // The §2.6 rendering is for the human at the gate.
            if let Some(text) = latest_quality_report_text(&ctx.store, report.task_id) {
                outln!("── quality report (platform-assembled) ──");
                outln!("{text}");
            }
            outln!("⏸ acceptance is a human gate — accept or return the result");
        }
        Gate::ReportFresh => {
            // The freshness gate stopped the run: no auditor execution was
            // started, no artifact was published. The human decides.
            let arts = ctx.store.artifacts(report.task_id)?;
            // REQ-005: selection by status alone — the store guarantees at
            // most one CURRENT per (task_id, kind) (schema v3 partial
            // unique index); no seq tie-break workaround here anymore.
            if let Some(a) = arts
                .iter()
                .find(|x| x.kind == ArtifactKind::QualityReport && x.status == ArtifactStatus::Current)
            {
                outln!(
                    "⏸ quality report v{} ({}) is FRESH — it is bound to exactly the recomputed workspace revision; the auditor was NOT re-run",
                    a.seq,
                    short(&a.id)
                );
            } else {
                outln!("⏸ quality report is FRESH for the current workspace revision — the auditor was NOT re-run");
            }
            if let Some(text) = latest_quality_report_text(&ctx.store, report.task_id) {
                outln!("── quality report (platform-assembled) ──");
                outln!("{text}");
            }
            outln!(
                "   decide explicitly: accept/return this report, or re-audit an unchanged workspace with `devai task run {} --force-reaudit`",
                short(&report.task_id)
            );
        }
        Gate::Terminal(st) => outln!("task finished: {}", st.as_str()),
        Gate::Hold(st) => outln!("task held in {} — release it first", st.as_str()),
    }
    let t = ctx.store.get_task(&report.task_id.to_string())?;
    let ops = workflow::allowed_ops(&t);
    if !ops.is_empty() {
        outln!("next: {}", ops.join(" | "));
    }
    Ok(())
}

/// The merge gate (REQ-WT-07): a plain `git merge` of the task's
/// platform-created `rework/<task-id>` branch into the workspace's current
/// baseline branch, performed ONLY because a human invoked this command.
/// This is deliberately the single merge path in the platform — the git
/// invocation lives in the CLI, behind a human subcommand, so no
/// engine/runtime code can reach it, and it rewrites nothing: fast-forward
/// or ordinary merge commit made with the HUMAN's git identity; a conflict
/// leaves the merge to the human, unrecorded. The target baseline is the
/// branch checked out when the command runs (OQ-9: the business does not
/// fix a configured baseline — what the human has checked out is theirs to
/// merge into). Task state is untouched by design: the merge is a repository
/// act, recorded as a decision + event, not a workflow transition.
fn task_merge(ctx: &Ctx, id: &str, workspace: Option<&Path>, rationale: Option<&str>, json: bool) -> Result<()> {
    let task = ctx.store.resolve_task(id)?;
    let repo = match workspace {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("no current directory; pass --workspace")?,
    };
    if !worktree::is_git_repo(&repo) {
        bail!("{} is not a git repository — nothing to merge into", repo.display());
    }
    let branch = worktree::branch_name(task.id);
    if !worktree::has_branch(&repo, task.id) {
        bail!("task {} has no `{branch}` branch — a writing stage creates it via the platform worktree; there is nothing to merge", short(&task.id));
    }
    let head_before = git_in(&repo, &["rev-parse", "--verify", "HEAD"]).context("baseline checkout has no commits to merge into")?;
    git_in(&repo, &["merge", "--no-edit", &branch])?;
    let head_after = git_in(&repo, &["rev-parse", "HEAD"])?;

    ctx.store.record_decision(&Decision {
        id: new_id(),
        task_id: task.id,
        kind: decision_kind::REWORK_MERGE.into(),
        verdict: Verdict::Approved,
        actor: ctx.actor.clone(),
        rationale: rationale.map(|s| s.to_string()),
        refs: vec![branch.clone(), head_after.clone()],
        created_at: ctx.now(),
    })?;
    ctx.store.insert_event(
        Some(task.id),
        event_kind::REWORK_MERGED,
        &ctx.actor,
        serde_json::json!({
            "branch": branch,
            "workspace": repo.display().to_string(),
            "head_before": head_before,
            "head_after": head_after,
        }),
    )?;
    if json {
        outln!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "task": task.id.to_string(), "branch": branch, "workspace": repo.display().to_string(),
            "head_before": head_before, "head_after": head_after,
        }))?);
    } else {
        outln!("merged `{branch}` into {}  {head_before} → {head_after}", repo.display());
        outln!("branch and worktree persist — their removal is a lifecycle decision the platform does not automate (OQ-1)");
    }
    Ok(())
}

/// The EMPTY-REWORK GATE escape (REQ-RG-03), human-only like the merge gate.
/// Records (or reuses) a `rework_noop_ratified` Decision for the exact report
/// pair named by the most recent `qa.rework_gate_tripped` event of the latest
/// report, releases the gate's blocking question, and runs the withheld
/// REWORK → QA transition through the same core command
/// (`implementation_reported`), which now finds the pair-bound ratification
/// and cites its decision id on the transition event (AC3). No engine or
/// agent path reaches this handler — the decision can only be created by a
/// human invoking the CLI (AC2), and there is no timeout auto-pass (AC
/// constraint): without the explicit act the task simply stays parked.
fn ratify_empty_rework(ctx: &Ctx, id: &str, rationale: Option<&str>, json: bool) -> Result<()> {
    let task = ctx.store.resolve_task(id)?;
    match task.state {
        // The gate hold is WAITING_FOR_INPUT returning to REWORK. If the
        // operator answered the question first (back to REWORK, no
        // ratification) — or a previous run died between the two acts —
        // ratifying from REWORK converges here too. Never from a hold of a
        // different question.
        TaskState::WaitingForInput if task.return_state == Some(TaskState::Rework) => {}
        TaskState::Rework => {}
        s => bail!(
            "ratify-empty-rework applies to the empty-rework gate (REWORK, or WAITING_FOR_INPUT returning to REWORK); the task is in {}",
            s.as_str()
        ),
    }
    let arts = ctx.store.artifacts(task.id)?;
    let Some(current) = arts.iter().filter(|a| a.kind == ArtifactKind::ImplementationReport).max_by_key(|a| a.seq) else {
        bail!("the task has no implementation report — nothing to ratify");
    };
    let events = ctx.store.all_events_for_task(task.id)?;
    let current_id_s = current.id.to_string();
    // Only the trip of the LATEST report is ratifiable: a newer report
    // changes the pair, and the older cycle's ratification is not lifetime
    // (REQ-RG-03 AC4) — the new publication tripped its own gate or passed.
    let trip = events
        .iter()
        .rev()
        .find(|e| {
            e.kind == event_kind::QA_REWORK_GATE_TRIPPED
                && e.payload.get(json_field::CURRENT_REPORT).and_then(|v| v.as_str()) == Some(current_id_s.as_str())
        })
        .ok_or_else(|| anyhow::anyhow!(
            "no empty-rework gate recorded for report {} — the gate fires when a REWORK publication names the unchanged commit; if the latest report is newer, let the engine publish it from REWORK first",
            short(&current.id)
        ))?;
    let prev_report = trip
        .payload
        .get(json_field::PREV_REPORT)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .context("the gate trip event names no previous report — history corruption")?
        .to_string();

    // The ratification decision for exactly this pair. Recorded once; a retry
    // reuses the stored row (this command must never double-write a judgment,
    // and it stays crash-safe between its commits).
    let decisions = ctx.store.list_decisions(task.id)?;
    let existing = decisions
        .iter()
        .find(|d| {
            d.kind == decision_kind::REWORK_NOOP_RATIFIED
                && d.refs.iter().any(|r| r == &prev_report)
                && d.refs.iter().any(|r| r == &current_id_s)
        })
        .cloned();
    let (decision, recorded_now) = match existing {
        Some(d) => (d, false),
        None => {
            let d = Decision {
                id: new_id(),
                task_id: task.id,
                kind: decision_kind::REWORK_NOOP_RATIFIED.into(),
                verdict: Verdict::Approved,
                actor: ctx.actor.clone(),
                rationale: rationale.map(|s| s.to_string()),
                refs: vec![prev_report.clone(), current_id_s.clone()],
                created_at: ctx.now(),
            };
            ctx.store.record_decision(&d)?;
            (d, true)
        }
    };

    // Release the blocking gate question while the hold is still up.
    let mut cur = task.clone();
    if cur.state == TaskState::WaitingForInput {
        let q_ref = trip.payload.get(json_field::QUESTION_ID).and_then(|v| v.as_str()).unwrap_or_default();
        let q = ctx.store.get_question(q_ref)?;
        if q.status == QuestionStatus::Open {
            ctx.store.answer_question(q.id, &format!("operator ratified the empty rework (decision {})", decision.id), &ctx.actor, ctx.now())?;
        }
        let m = workflow::answer_question(&cur, q.id, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
        ctx.store.commit(cur.lock_version, &m)?;
        cur = ctx.store.get_task(&cur.id.to_string())?;
    }

    // Run the withheld transition through the single serialization point.
    // With the stored ratification now in the facts, the core command admits
    // REWORK → QA and puts `ratified_by` on the event (REQ-RG-03 AC3).
    let arts = ctx.store.artifacts(cur.id)?;
    let facts = devai_runtime::gate::collect_implementation_report_facts(
        &ctx.store,
        cur.id,
        current,
        &arts,
        Some(format!("empty rework ratified by decision {}", decision.id)),
    )?;
    let m = workflow::implementation_reported(&cur, &facts, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
    if m.task.state != TaskState::Qa {
        bail!(
            "the withheld transition did not land in QA (state {}); decision {} stands — inspect `devai task events {}`",
            m.task.state.as_str(),
            decision.id,
            short(&cur.id)
        );
    }
    ctx.store.commit(cur.lock_version, &m)?;
    if json {
        outln!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "task": cur.id.to_string(),
            "decision": decision.id.to_string(),
            "recorded_now": recorded_now,
            "prev_report": prev_report,
            "current_report": current_id_s,
            "state": m.task.state.as_str(),
        }))?);
    } else {
        outln!("ratified: decision {} ({}) — the no-op rework is now a conscious operator passage", decision.id, if recorded_now { "recorded" } else { "reused stored" });
        outln!("pair: report {prev_report} → report {current_id_s} on the same commit");
        outln!("state: {} → QA; the transition event cites the decision", cur.state.as_str());
        outln!("next: {}", workflow::allowed_ops(&m.task).join(" | "));
    }
    Ok(())
}

/// Read-only git plumbing for the merge gate: the CLI never grows domain
/// logic, it only performs exactly the one repository act a human ordered.
fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().with_context(|| format!("launch git in {}", dir.display()))?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The human gate's viewing channel: the CURRENT quality report. A verdict
/// that must be signed must also be readable — after the run printed it, on
/// demand, forever (history is never deleted; only CURRENT is shown).
fn show_report(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    let t = ctx.store.resolve_task(id)?;
    let arts = ctx.store.artifacts(t.id)?;
    // REQ-005: the CURRENT quality report is found by status alone — the
    // domain keeps at most one CURRENT per (task_id, kind), so there is no
    // seq tie-break to do here (the old highest-seq selection masked
    // multi-CURRENT corruption at the UI instead of letting it surface as
    // a domain bug).
    let a = arts
        .iter()
        .find(|x| x.kind == ArtifactKind::QualityReport && x.status == ArtifactStatus::Current)
        .with_context(|| format!("no quality report for {} yet — QA has not run", short(&t.id)))?;
    let blob = ctx.store.read_blob(&a.sha256)?;
    let contract = a
        .contract
        .as_ref()
        .map(|c| format!("{}@{}", c.name, c.version))
        .unwrap_or_else(|| "—".into());
    let value: serde_json::Value = serde_json::from_slice(&blob).unwrap_or(serde_json::Value::Null);
    if json {
        outln!(
            "{}",
            serde_json::json!({
                "task": t.id.to_string(),
                "artifact": a.id.to_string(),
                "seq": a.seq,
                "contract": contract,
                "created_at": a.created_at,
                "execution": a.execution_id.map(|e| e.to_string()),
                "report": value,
            })
        );
        return Ok(());
    }
    outln!(
        "quality report v{}  artifact {}  contract {contract}  execution {}",
        a.seq,
        short(&a.id),
        a.execution_id.map(|e| short(&e)).unwrap_or_else(|| "-".into())
    );
    outln!("---");
    match value.get("report_text").and_then(|s| s.as_str()) {
        Some(text) => outln!("{text}"),
        // Reports assembled before @2 carry no rendering: show the machine
        // JSON rather than pretend there is nothing to read.
        None => outln!("{}", serde_json::to_string_pretty(&value).unwrap_or_else(|_| String::from_utf8_lossy(&blob).into_owned())),
    }
    Ok(())
}

fn latest_quality_report_text(store: &devai_store::Store, task_id: devai_core::types::Id) -> Option<String> {
    let arts = store.artifacts(task_id).ok()?;
    // REQ-005: status-only selection (single CURRENT pinned at v3).
    let a = arts
        .iter()
        .find(|a| a.kind == devai_core::types::ArtifactKind::QualityReport && a.status == devai_core::types::ArtifactStatus::Current)?;
    let blob = store.read_blob(&a.sha256).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&blob).ok()?;
    v.get("report_text")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .or_else(|| Some("(report assembled before @2 — no rendering)".into()))
}

fn revise(ctx: &Ctx, id: &str, file: &Path, reason: Option<&str>) -> Result<()> {
    let task = ctx.store.resolve_task(id)?;
    let (kind, name) = (ArtifactKind::RequirementsSpec, file.display().to_string());
    let content = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    let _a = publish_raw(ctx, task.id, kind, &name, content, None, None, vec![], None, DataCategory::Artifact)?;
    let after = ctx.store.artifacts(task.id)?;
    let m = workflow::requirements_published(&task, &after, reason, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
    ctx.store.commit(task.lock_version, &m)?;
    outln!("requirements v{} published → REVIEW", m.task.requirements_seq);
    let staled: Vec<_> = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Stale).map(|(i, _)| i.to_string()).collect();
    if !staled.is_empty() {
        outln!("consequences: {} artifact(s) marked STALE:", staled.len());
        for s in &staled {
            outln!("  stale  {s}");
        }
        outln!("old versions are kept in history; nothing was deleted");
    }
    outln!("next: {}", workflow::allowed_ops(&m.task).join(" | "));
    Ok(())
}

fn status(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    let t = ctx.store.resolve_task(id)?;
    let artifacts = ctx.store.artifacts(t.id)?;
    let questions = ctx.store.list_questions(t.id)?;
    let open_q: Vec<_> = questions.iter().filter(|q| q.status == QuestionStatus::Open).collect();
    let current: Vec<_> = artifacts.iter().filter(|a| a.status == ArtifactStatus::Current).collect();
    let ops = workflow::allowed_ops(&t);
    if json {
        outln!(
            "{}",
            serde_json::json!({
                "task": t,
                "allowed_operations": ops,
                "current_artifacts": current,
                "open_questions": open_q,
            })
        );
    } else {
        print_task(&t);
        outln!("---");
        outln!("artifacts (current): {}", current.len());
        for a in &current {
            outln!("  {:<22} v{:<3} {}  {}", a.kind.as_str(), a.seq, short(&a.id), a.name);
        }
        for q in &open_q {
            outln!("open question {}: {}{}", short(&q.id), q.text, if q.blocking { " [BLOCKING]" } else { "" });
        }
        outln!(
            "allowed operations: {}",
            if ops.is_empty() { "(none — task is terminal)".to_string() } else { ops.join(" | ") }
        );
    }
    Ok(())
}

fn print_task(t: &Task) {
    outln!("id:        {}", t.id);
    outln!("title:     {}", t.title);
    outln!("state:     {}{}", t.state.as_str(), t.return_state.map(|r| format!(" (returns to {})", r.as_str())).unwrap_or_default());
    outln!("requirements: v{}", t.requirements_seq);
    outln!("project:   {}", t.project_id.map(|i| short(&i)).unwrap_or_else(|| "-".into()));
    outln!("lock:      {}", t.lock_version);
    outln!("created:   {}  updated: {}", t.created_at.format("%Y-%m-%d %H:%M:%S"), t.updated_at.format("%Y-%m-%d %H:%M:%S"));
    if !t.business_description.is_empty() {
        outln!("business:  {}", ellipsis(&t.business_description, 200));
    }
}

/// `devai task progress <ID>` — pipeline position at a glance. The view is
/// drawn purely from engine data (REQ-PROG-03): the spine order and the
/// position/re-entry come from `workflow::pipeline_*`, the operations from
/// `workflow::allowed_ops`, the running executions from
/// `store.list_executions`. The renderer owns no state-machine knowledge of
/// its own: change the engine's op set and the progress view changes with it.
fn progress(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    let t = ctx.store.resolve_task(id)?;
    let ops = workflow::allowed_ops(&t);
    let order = workflow::pipeline_order();
    let position = workflow::pipeline_position(&t);
    let pos_idx = position.and_then(|p| order.iter().position(|s| *s == p));
    let off_spine = workflow::pipeline_off_spine(&t);
    let now = ctx.now();
    let running: Vec<_> = ctx
        .store
        .list_executions(Some(t.id))?
        .into_iter()
        .filter(|e| e.status == ExecutionStatus::Running)
        .collect();

    if json {
        outln!(
            "{}",
            serde_json::json!({
                "task": t,
                "snapshot_at": now,
                "pipeline": {
                    "order": order.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    "current": t.state.as_str(),
                    "position": position.map(|s| s.as_str()),
                    "position_index": pos_idx,
                    "off_spine": off_spine,
                    "terminal": t.state.is_terminal(),
                },
                "allowed_operations": ops,
                "running_executions": running
                    .iter()
                    .map(|e| serde_json::json!({
                        "id": e.id.to_string(),
                        "role": format!("{}@{}", e.role_slug, e.role_version),
                        "role_slug": e.role_slug,
                        "role_version": e.role_version,
                        "agent_name": e.agent_name,
                        "started_at": e.started_at,
                        "elapsed_seconds": (now - e.started_at).num_seconds().max(0),
                    }))
                    .collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }

    outln!("id:    {}", t.id);
    outln!("title: {}", t.title);
    outln!(
        "state: {}{}",
        t.state.as_str(),
        t.return_state.map(|r| format!(" (returns to {})", r.as_str())).unwrap_or_default()
    );
    let mut nodes = Vec::with_capacity(order.len());
    for (i, s) in order.iter().enumerate() {
        let name = s.as_str();
        if Some(i) == pos_idx && !t.state.is_terminal() {
            nodes.push(format!("[{name}]"));
        } else if pos_idx.is_some_and(|p| p > i) {
            nodes.push(format!("✓ {name}"));
        } else {
            nodes.push(name.to_string());
        }
    }
    outln!("pipeline: {}", nodes.join(" → "));
    if t.state.is_terminal() {
        outln!("cycle ended in {} — terminal state, no open pipeline step", t.state.as_str());
    } else if off_spine {
        // REWORK loop / hold state: the bracket marks where it re-enters.
        match position {
            Some(p) => outln!("note:  {} is off the spine — it returns to [{}]", t.state.as_str(), p.as_str()),
            None => outln!("note:  {} holds no remembered stage (state corruption — check `devai task events`)", t.state.as_str()),
        }
    }
    outln!("---");
    outln!("running executions:");
    if running.is_empty() {
        outln!("  (no running executions)");
    } else {
        for e in &running {
            outln!(
                "  {:<24} {:<12} elapsed {}",
                format!("{}@{}", e.role_slug, e.role_version),
                short(&e.id),
                human_elapsed((now - e.started_at).num_seconds())
            );
        }
    }
    outln!(
        "allowed operations: {}",
        if ops.is_empty() { "(none — task is terminal)".to_string() } else { ops.join(" | ") }
    );
    Ok(())
}

/// Readable elapsed from a raw seconds count: `42s`, `3m 07s`, `2h 05m`.
fn human_elapsed(total: i64) -> String {
    let s = total.max(0);
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

// ---------------------------------------------------------------------------
// artifacts
// ---------------------------------------------------------------------------

fn artifact(ctx: &Ctx, cmd: ArtifactCmd, json: bool) -> Result<()> {
    match cmd {
        ArtifactCmd::Publish { id, kind, file, name, depends, contract, execution, verdict, category } => {
            let kind = ArtifactKind::parse(&kind.replace('-', "_")).map_err(anyhow::Error::new)?;
            let category = DataCategory::parse(&category.replace('-', "_")).map_err(anyhow::Error::new)?;
            let task = ctx.store.resolve_task(&id)?;
            let content = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            let mut depends_ids = resolve_ids(ctx, &depends)?;
            // Downstream role results are implicitly children of the current
            // approved specification, so a later requirements revision marks
            // them STALE through the dependency graph (unless deps were given).
            if depends.is_empty() && is_downstream_of_spec(kind) {
                if let Some(spec) = current_spec_id(&ctx.store.artifacts(task.id)?) {
                    depends_ids.push(spec);
                }
            }
            let execution_id = match execution.as_deref() {
                Some(e) => Some(ctx.store.get_execution(e)?.id),
                None => None,
            };
            let created_by = match &execution_id {
                Some(_) => format!("execution:{}", execution_id.unwrap()),
                None => ctx.actor.clone(),
            };
            let contract_ref = match contract.as_deref() {
                Some(c) => Some(ContractRef::parse(c).map_err(anyhow::Error::new)?),
                None => None,
            };
            let a = publish_raw(ctx, task.id, kind, &name.unwrap_or_else(|| file.display().to_string()), content, contract_ref, execution_id, depends_ids, Some(&created_by), category)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&a)?);
            } else {
                outln!("artifact {} {} v{} ({})", a.kind.as_str(), short(&a.id), a.seq, a.name);
            }
            // Some kinds carry the workflow forward.
            match kind {
                ArtifactKind::RequirementsSpec => {
                    let arts = ctx.store.artifacts(task.id)?;
                    let m = workflow::requirements_published(&task, &arts, None, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
                    ctx.store.commit(task.lock_version, &m)?;
                    outln!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
                }
                ArtifactKind::ReviewReport => {
                    let v = match verdict.as_deref() {
                        Some(s) => parse_verdict(s)?,
                        None => bail!("publishing REVIEW_REPORT requires --verdict approved|needs_revision"),
                    };
                    let m = workflow::review_recorded(&task, v, None, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
                    ctx.store.commit(task.lock_version, &m)?;
                    outln!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
                }
                ArtifactKind::ImplementationReport => {
                    // The manual publication crosses the SAME core command as
                    // the engine — including the empty-rework gate: no
                    // producer path may bypass it (REQ-RG-01 constraint).
                    let arts = ctx.store.artifacts(task.id)?;
                    let facts = devai_runtime::gate::collect_implementation_report_facts(&ctx.store, task.id, &a, &arts, None)?;
                    let m = workflow::implementation_reported(&task, &facts, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
                    ctx.store.commit(task.lock_version, &m)?;
                    outln!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
                    if m.task.state == TaskState::WaitingForInput {
                        outln!("⏸ EMPTY-REWORK GATE: report {} names the SAME commit as the previous one — the rework captured no new work; QA is withheld.", short(&a.id));
                        outln!("   operator: ratify the no-op explicitly with `devai task ratify-empty-rework {} --rationale \"...\"`, or answer the question and re-run the implementation with real fixes", short(&task.id));
                    }
                }
                ArtifactKind::QualityReport => {
                    outln!("quality report recorded; user decides via `devai task accept|return`");
                }
                _ => {}
            }
        }
        ArtifactCmd::List { id } => {
            let t = ctx.store.resolve_task(&id)?;
            let as_ = ctx.store.artifacts(t.id)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&as_)?);
            } else if as_.is_empty() {
                outln!("(no artifacts)");
            } else {
                for a in as_ {
                    outln!("{}  {:<22} v{:<3} {:<12} {}  {}", short(&a.id), a.kind.as_str(), a.seq, a.status.as_str(), a.created_by, a.name);
                }
            }
        }
        ArtifactCmd::Cat { artifact_id } => {
            let a = ctx.store.get_artifact(&artifact_id)?;
            let bytes = ctx.store.read_blob(&a.sha256)?;
            // Central raw-bytes path: a closed reader dies quietly (REQ-PIPE-01).
            crate::out::write_bytes_all(&bytes)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn publish_raw(
    ctx: &Ctx,
    task_id: Id,
    kind: ArtifactKind,
    name: &str,
    content: Vec<u8>,
    contract: Option<ContractRef>,
    execution: Option<Id>,
    depends: Vec<Id>,
    created_by: Option<&str>,
    category: DataCategory,
) -> Result<Artifact> {
    if let Some(c) = &contract {
        if ctx.store.get_contract(&c.name, c.version)?.is_none() {
            bail!("contract {c} is not registered (devai contract add)");
        }
    }
    Ok(ctx.store.publish_artifact(ArtifactDraft {
        task_id,
        kind,
        name: name.to_string(),
        category,
        media_type: guess_media_type(Path::new(name)),
        content,
        contract,
        created_by: created_by.unwrap_or(&ctx.actor).to_string(),
        execution_id: execution,
        depends_on: depends,
    })?)
}

// ---------------------------------------------------------------------------
// questions
// ---------------------------------------------------------------------------

fn question(ctx: &Ctx, cmd: QuestionCmd, json: bool) -> Result<()> {
    match cmd {
        QuestionCmd::Ask { id, text, context, options, blocking, execution } => {
            let task = ctx.store.resolve_task(&id)?;
            let execution_id = match execution.as_deref() {
                Some(e) => Some(ctx.store.get_execution(e)?.id),
                None => None,
            };
            let q = Question {
                id: new_id(),
                task_id: task.id,
                execution_id,
                text,
                context,
                options,
                blocking,
                status: QuestionStatus::Open,
                answer: None,
                answered_by: None,
                answered_at: None,
                created_at: ctx.now(),
            };
            ctx.store.create_question(&q)?;
            let m = workflow::raise_question(&task, q.id, blocking, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
            ctx.store.commit(task.lock_version, &m)?;
            if blocking && m.task.state != task.state {
                outln!("question {} raised; task parked in WAITING_FOR_INPUT", short(&q.id));
                outln!("answer with: devai question answer {}", short(&q.id));
            } else {
                outln!("question {} recorded", short(&q.id));
            }
        }
        QuestionCmd::Answer { question_id, text } => {
            let q = ctx.store.get_question(&question_id)?;
            if q.status == QuestionStatus::Answered {
                bail!("question {} is already answered", q.id);
            }
            let task = ctx.store.get_task(&q.task_id.to_string())?;
            ctx.store.answer_question(q.id, &text, &ctx.actor, ctx.now())?;
            let m = workflow::answer_question(&task, q.id, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
            ctx.store.commit(task.lock_version, &m)?;
            outln!("answer recorded; state: {} → {}", task.state.as_str(), m.task.state.as_str());
        }
        QuestionCmd::List { id } => {
            let t = ctx.store.resolve_task(&id)?;
            let qs = ctx.store.list_questions(t.id)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&qs)?);
            } else if qs.is_empty() {
                outln!("(no questions)");
            } else {
                for q in qs {
                    outln!("{} {:<8} {}{}", short(&q.id), q.status.as_str(), q.text, if q.blocking && q.status == QuestionStatus::Open { " [BLOCKING]" } else { "" });
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// roles / agents / contracts
// ---------------------------------------------------------------------------

fn role(ctx: &Ctx, cmd: RoleCmd, json: bool) -> Result<()> {
    match cmd {
        RoleCmd::Add { file } => {
            let body = std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
            let mut r: RoleDefinition = serde_json::from_str(&body).context("invalid RoleDefinition JSON")?;
            // version override: latest+1 for this slug if caller left 0
            if r.version == 0 {
                r.version = ctx.store.get_role(&r.slug)?.map(|e| e.version + 1).unwrap_or(1);
            }
            if r.id == Uuid::nil() {
                r.id = new_id();
            }
            r.created_at = ctx.now();
            ctx.store.put_role(&r)?;
            outln!("role {}@{} registered", r.slug, r.version);
        }
        RoleCmd::List => {
            let rs = ctx.store.list_roles()?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&rs)?);
            } else if rs.is_empty() {
                outln!("(no roles — run `devai init`)");
            } else {
                for r in rs {
                    outln!("{:<24} v{:<3} {}  in={} out={}", r.slug, r.version, r.name,
                        r.input_contract.map(|c| c.to_string()).unwrap_or("-".into()),
                        r.output_contract.map(|c| c.to_string()).unwrap_or("-".into()));
                }
            }
        }
        RoleCmd::Get { slug } => match ctx.store.get_role(&slug)? {
            Some(r) => outln!("{}", serde_json::to_string_pretty(&r)?),
            None => bail!("role `{slug}` not found"),
        },
    }
    Ok(())
}

fn agent(ctx: &Ctx, cmd: AgentCmd, json: bool) -> Result<()> {
    match cmd {
        AgentCmd::Add { name, provider, model, runtime } => {
            let a = AgentInstance { id: new_id(), name: name.clone(), provider, model, runtime, created_at: ctx.now() };
            ctx.store.put_agent(&a)?;
            outln!("agent {name} registered (credentials stay outside the store)");
        }
        AgentCmd::List => {
            let as_ = ctx.store.list_agents()?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&as_)?);
            } else {
                for a in as_ {
                    outln!("{}  provider={}  model={}  runtime={}", a.name, a.provider, a.model, a.runtime);
                }
            }
        }
    }
    Ok(())
}

fn contract(ctx: &Ctx, cmd: ContractCmd, json: bool) -> Result<()> {
    match cmd {
        ContractCmd::Add { name, version, file } => {
            let schema = std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
            serde_json::from_str::<serde_json::Value>(&schema).context("contract file must be valid JSON")?;
            ctx.store.put_contract(&name, version, &schema)?;
            outln!("contract {name}@{version} registered");
        }
        ContractCmd::Get { name, version } => match ctx.store.get_contract(&name, version)? {
            Some(s) => {
                if json {
                    outln!("{s}");
                } else {
                    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
                    outln!("{}", serde_json::to_string_pretty(&v)?);
                }
            }
            None => bail!("contract {name}@{version} not found"),
        },
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// role executions
// ---------------------------------------------------------------------------

fn exec(ctx: &Ctx, cmd: ExecCmd, json: bool) -> Result<()> {
    match cmd {
        ExecCmd::Start { id, role, agent, inputs } => {
            let task = ctx.store.resolve_task(&id)?;
            workflow::check_execution_allowed(&role, &task).map_err(anyhow::Error::new)?;
            let rd = ctx.store.get_role(&role)?;
            let version = rd.as_ref().map(|r| r.version).unwrap_or(1);
            let inputs_str: Vec<String> = resolve_ids(ctx, &inputs)?.iter().map(|i| i.to_string()).collect();
            // REQ-001/REQ-006: even a manually started execution binds to the
            // workspace state the platform can observe right now (cwd),
            // computed platform-side — never supplied by the caller.
            let revision = Engine::new(&ctx.store).current_revision().map_err(anyhow::Error::new)?;
            let e = RoleExecution {
                id: new_id(),
                task_id: task.id,
                role_slug: role.clone(),
                role_version: version,
                agent_name: agent,
                status: ExecutionStatus::Running,
                inputs: inputs_str,
                summary: None,
                error: None,
                started_at: ctx.now(),
                finished_at: None,
                workspace_revision: Some(revision),
            };
            ctx.store.start_execution(&e)?;
            outln!("execution {} for role {role}@{version} started in state {}", short(&e.id), task.state.as_str());
            outln!("publish its output artifact with: devai artifact publish {} --execution {}", short(&task.id), short(&e.id));
        }
        ExecCmd::Finish { exec_id, summary } => {
            let e = ctx.store.get_execution(&exec_id)?;
            if e.status != ExecutionStatus::Running {
                bail!("execution {} is {}, not RUNNING", e.id, e.status.as_str());
            }
            ctx.store.finish_execution(&e, ExecutionStatus::Completed, summary.as_deref(), None, ctx.now())?;
            outln!("execution {} completed", short(&e.id));
        }
        ExecCmd::Fail { exec_id, error } => {
            let e = ctx.store.get_execution(&exec_id)?;
            if e.status != ExecutionStatus::Running {
                bail!("execution {} is {}, not RUNNING", e.id, e.status.as_str());
            }
            ctx.store.finish_execution(&e, ExecutionStatus::Failed, None, Some(&error), ctx.now())?;
            outln!("execution {} failed (task workflow state unchanged; the stage may be retried)", short(&e.id));
        }
        ExecCmd::Cancel { exec_id } => {
            let e = ctx.store.get_execution(&exec_id)?;
            if e.status != ExecutionStatus::Running {
                bail!("execution {} is {}, not RUNNING", e.id, e.status.as_str());
            }
            ctx.store.finish_execution(&e, ExecutionStatus::Cancelled, None, None, ctx.now())?;
            outln!("execution {} cancelled (cooperative stop acknowledged)", short(&e.id));
        }
        ExecCmd::List { task } => {
            let tid = match task.as_deref() {
                Some(id) => Some(ctx.store.resolve_task(id)?.id),
                None => None,
            };
            let es = ctx.store.list_executions(tid)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&es)?);
            } else if es.is_empty() {
                outln!("(no executions)");
            } else {
                for e in es {
                    outln!("{}  {:<22} {:<12} {:<10} {}", short(&e.id), e.role_slug, short(&e.task_id), e.status.as_str(),
                        e.summary.as_deref().or(e.error.as_deref()).map(|s| ellipsis(s, 60)).unwrap_or_default());
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// export / import
// ---------------------------------------------------------------------------

fn export(ctx: &Ctx, id: &str, out: &Path, json: bool) -> Result<()> {
    let t = ctx.store.resolve_task(id)?;
    let manifest = devai_store::bundle::export(&ctx.store, &t, out)?;
    if json {
        outln!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        outln!("bundle exported to {}", out.display());
        outln!("task {} — {} events, {} artifacts, {} executions, {} roles, {} contracts",
            short(&manifest.task_id), manifest.counts.events, manifest.counts.artifacts,
            manifest.counts.executions, manifest.counts.roles, manifest.counts.contracts);
        outln!("manifest carries sha256 for every file; caches and credentials are never included");
    }
    Ok(())
}

fn import(ctx: &Ctx, dir: &Path, json: bool) -> Result<()> {
    let o = devai_store::bundle::import(&ctx.store, dir)?;
    if json {
        outln!("{}", serde_json::json!({"task_id": o.task_id.to_string(), "artifacts": o.imported_artifacts, "events": o.imported_events, "skipped_existing": o.skipped_existing}));
    } else if o.skipped_existing {
        outln!("task {} already present — nothing imported", short(&o.task_id));
    } else {
        outln!("task {} imported: {} artifacts, {} events restored", short(&o.task_id), o.imported_artifacts, o.imported_events);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Kinds that are derived from the approved requirements and should be
/// marked STALE when requirements are revised.
fn is_downstream_of_spec(kind: ArtifactKind) -> bool {
    matches!(
        kind,
        ArtifactKind::ReviewReport
            | ArtifactKind::ImplementationPlan
            | ArtifactKind::ImplementationReport
            | ArtifactKind::Patch
            | ArtifactKind::TestReport
            | ArtifactKind::QualityReport
    )
}

/// The CURRENT requirements spec, if any — by status alone (REQ-005: the
/// store pins at most one CURRENT per (task_id, kind) since schema v3).
fn current_spec_id(arts: &[Artifact]) -> Option<Id> {
    arts.iter()
        .find(|a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current)
        .map(|a| a.id)
}

fn resolve_ids(ctx: &Ctx, prefixes: &[String]) -> Result<Vec<Id>> {
    let mut out = Vec::new();
    for p in prefixes {
        let a = ctx.store.get_artifact(p)?;
        out.push(a.id);
    }
    Ok(out)
}

fn parse_verdict(s: &str) -> Result<Verdict> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "approved" | "approve" | "pass" => Verdict::Approved,
        "needs_revision" | "revision" | "changes" => Verdict::NeedsRevision,
        "rejected" | "reject" => Verdict::Rejected,
        "returned" | "return" => Verdict::Returned,
        other => bail!("unknown verdict `{other}` (approved|needs_revision|rejected|returned)"),
    })
}

pub(crate) fn short(id: &Uuid) -> String {
    // UUIDv7's leading bytes encode the creation timestamp, so very short
    // prefixes collide for tasks created in the same instant. 12 chars keep
    // the display compact while remaining practically unique; resolve_task
    // still fails loudly on any genuine ambiguity (never silently overwrites).
    id.to_string()[..12].to_string()
}

fn ellipsis(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n {
        s.to_string()
    } else {
        format!("{}…", c[..n].iter().collect::<String>())
    }
}

fn compact(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
        other => other.to_string(),
    };
    ellipsis(&s, 120)
}
