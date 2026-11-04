use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_runtime::{liveness, worktree, Engine, Gate};
use devai_store::{ArtifactDraft, Store, guess_media_type};
use uuid::Uuid;

use crate::outln;
use crate::{ApiCmd, ArtifactCmd, AgentCmd, ContractCmd, ExecCmd, ProjectCmd, QuestionCmd, RoleCmd, TaskCmd, Cmd};
use devai_api::Api;
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
        // REQ-API-03 C2: Store::open here is the BOOTSTRAP entry point — the
        // home directory is opened before any operation runs. It is not an
        // operation path: every command dispatches through `Ctx::api` below.
        let store = Store::open(&dir).context("open local store")?;
        Ok(Ctx { home: dir, actor, store })
    }

    /// The platform API surface this CLI consumes as its first consumer
    /// (REQ-API-03, ratified OQ-1: in-process crate). Commands routed through
    /// it call typed operations here; direct `ctx.store` reads are the
    /// RATIFIED OQ-10 exception (init/seed bootstrap) and the cycle-2
    /// mutation paths pending migration (REQ-API-11) — marked at each site.
    pub fn api(&self) -> Api<'_> {
        Api::new(&self.store, &self.home, &self.actor)
    }

    fn now(&self) -> Timestamp {
        Utc::now()
    }
}

/// Render an API error the way the CLI rendered the store error before the
/// API layer existed: `devai: <message>` on stderr, nonzero exit
/// (REQ-API-04 AC2 — an API failure is an explicit message, never an empty
/// or truncated output). `ApiError` Display forwards the inner text, so the
/// human-visible message is byte-identical to the pre-API path.
pub fn render_api_error(e: devai_api::ApiError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

pub fn run(ctx: &Ctx, cmd: Cmd, json: bool) -> Result<()> {
    match cmd {
        // RATIFIED OQ-10 EXCEPTION (REQ-API-05 AC3, marked per AC2):
        // init/seed keep the direct store path — bootstrap runs against a
        // home that may not exist yet and cannot be expressed through the
        // API. The pair stays together by ratification.
        Cmd::Init { config } => init(ctx, config.as_deref(), json),
        Cmd::Project(c) => project(ctx, c, json),
        Cmd::Task(c) => task(ctx, c, json),
        Cmd::Artifact(c) => artifact(ctx, c, json),
        Cmd::Question(c) => question(ctx, c, json),
        Cmd::Role(c) => role(ctx, c, json),
        Cmd::Agent(c) => agent(ctx, c, json),
        Cmd::Contract(c) => contract(ctx, c, json),
        Cmd::Exec(c) => exec(ctx, c, json),
        Cmd::Api(c) => api_registry(ctx, c, json),
        Cmd::Export { task, out } => export(ctx, &task, &out, json),
        Cmd::Import { dir } => import(ctx, &dir, json),
    }
}

// ---------------------------------------------------------------------------
// api registry (REQ-API-06 AC4: the published contract, one command)
// ---------------------------------------------------------------------------

fn api_registry(ctx: &Ctx, cmd: ApiCmd, json: bool) -> Result<()> {
    match cmd {
        ApiCmd::Registry => {
            let ops = ctx.api().api_registry();
            if json {
                outln!("{}", serde_json::to_string_pretty(&ops)?);
            } else {
                for o in &ops {
                    let class = match o.class {
                        devai_api::OpClass::Read => "read",
                        devai_api::OpClass::Mutation => "mutation",
                    };
                    outln!("{}  [{class}]  in={}  out={}", o.name, o.input, o.output);
                    outln!("    {}", o.description);
                }
                outln!("--- {} operations published (RATIFIED OQ-10 init/seed stay a direct-store bootstrap exception, not API operations)", ops.len());
            }
        }
    }
    Ok(())
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
            let ps = ctx.api().project_list().map_err(render_api_error)?;
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
        ProjectCmd::Get { id } => match ctx.api().project_get(&id).map_err(render_api_error)? {
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

/// Cycle-2 mark (REQ-API-11): handlers whose READ facts below still resolve
/// the task through `ctx.store` carry a CYCLE-2 PENDING comment; every pure
/// read path routes through `ctx.api()` (REQ-API-03 AC1 for the read plane).
fn task(ctx: &Ctx, cmd: TaskCmd, json: bool) -> Result<()> {
    match cmd {
        TaskCmd::New { title, project, description, description_file, parent, blocked_by } => {
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
            // REQ-DEP-07: a derived task records WHY it exists at the moment
            // it is created — the genealogy/blocking never leaves for the
            // operator chat. Dangling references fail loudly here (resolve),
            // exactly as they would via `task depend`.
            let mut linked: Vec<TaskDependency> = Vec::new();
            for p in &parent {
                let pt = ctx.store.resolve_task(p)?;
                linked.push(ctx.store.link_task_dependency(t.id, pt.id, DependencyKind::Parent, &ctx.actor, ctx.now())?);
            }
            for b in &blocked_by {
                let bt = ctx.store.resolve_task(b)?;
                linked.push(ctx.store.link_task_dependency(t.id, bt.id, DependencyKind::Blocks, &ctx.actor, ctx.now())?);
            }
            if json {
                outln!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                outln!("task {} created in DRAFT — workflow is owned by devai", short(&t.id));
                for d in &linked {
                    outln!("  depends {} {} ({} in {})", d.kind.as_str(), short(&d.depends_on_id), d.depends_on_state.as_str(), d.depends_on_title);
                }
                outln!("start it with: devai task start {}", short(&t.id));
            }
        }
        TaskCmd::List => {
            let ts = ctx.api().task_list().map_err(render_api_error)?;
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
            let t = ctx.api().task_get(&id).map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                print_task(&t);
            }
        }
        TaskCmd::Status { id } => status(ctx, &id, json)?,
        TaskCmd::Progress { id } => progress(ctx, &id, json)?,
        TaskCmd::Active => active_overview(ctx, json)?,
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
        TaskCmd::Accept { id, rationale } => {
            // REQ-DEP-03: the human acceptance gate reads the stored
            // dependency graph — an open 'blocks' edge refuses the gate and
            // the refusal enumerates the blocking task ids as data. The one
            // release (RATIFIED DECISION OQ2) is the operator's explicit
            // `dependency_override` decision, read from stored decisions —
            // an edge closed means the blocker ACCEPTED, never merely
            // cancelled/failed (RATIFIED DECISION OQ1).
            let tid = ctx.store.resolve_task(&id)?.id;
            let deps = ctx.store.task_dependencies(tid)?;
            let decisions = ctx.store.list_decisions(tid)?;
            op(ctx, &id, move |t, a| workflow::accept(t, a, &deps, &decisions, rationale.as_deref(), &ctx.actor, ctx.now()))?;
        }
        TaskCmd::Return { id, rationale } => op(ctx, &id, move |t, _a| workflow::return_to_rework(t, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::RatifyEmptyRework { id, rationale } => ratify_empty_rework(ctx, &id, rationale.as_deref(), json)?,
        TaskCmd::Merge { id, workspace, rationale } => task_merge(ctx, &id, workspace.as_deref(), rationale.as_deref(), json)?,
        TaskCmd::Pause { id, reason } => op(ctx, &id, move |t, _a| workflow::pause(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Resume { id } => {
            // REQ-DEP-03: resume from a hold state consults the stored graph
            // (open blockers enumerate into the refusal; "closed" = blocker
            // ACCEPTED per ratified OQ1). The only escape is the operator's
            // own `task override` ratification (ratified OQ2). Closing the
            // last blocker does NOT resume the task by itself — auto-unpause
            // is forbidden, resume stays a human command.
            let tid = ctx.store.resolve_task(&id)?.id;
            let deps = ctx.store.task_dependencies(tid)?;
            let decisions = ctx.store.list_decisions(tid)?;
            op(ctx, &id, move |t, _a| workflow::resume(t, &deps, &decisions, &ctx.actor, ctx.now()))?;
        }
        TaskCmd::Block { id, reason } => op(ctx, &id, move |t, _a| workflow::block(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Cancel { id, reason } => op(ctx, &id, move |t, _a| workflow::cancel(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Fail { id, reason } => op(ctx, &id, move |t, _a| workflow::fail(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Events { id, limit } => {
            let evs = ctx.api().task_events(&id, limit).map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&evs)?);
            } else {
                for e in evs {
                    outln!("{}  {:<28} {:<12} {}", e.created_at.format("%Y-%m-%d %H:%M:%S"), e.kind, e.actor, compact(&e.payload));
                }
            }
        }
        TaskCmd::Decisions { id } => {
            // New CLI surface over the API task.decisions read: the human
            // decision rows as data, available to any consumer.
            let ds = ctx.api().task_decisions(&id).map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&ds)?);
            } else if ds.is_empty() {
                outln!("(no decisions)");
            } else {
                for d in ds {
                    outln!("{}  {:<26} {:<10} by {}  {}", d.created_at.format("%Y-%m-%d %H:%M:%S"), d.kind, d.verdict.as_str(), d.actor, d.rationale.map(|s| ellipsis(&s, 100)).unwrap_or_default());
                    if !d.refs.is_empty() {
                        outln!("    refs: {}", d.refs.join(", "));
                    }
                }
            }
        }
        TaskCmd::Depend { id, on, kind } => {
            let kind = DependencyKind::parse(&kind).map_err(anyhow::Error::new)?;
            let task = ctx.store.resolve_task(&id)?;
            let depends_on = ctx.store.resolve_task(&on)?;
            // Registration is LOUD at write (REQ-DEP-07 AC2): self-reference,
            // a duplicate edge, a dangling id or a cycle WITHIN one kind
            // (ratified OQ4: parent-DAG and blocks-DAG; a mixed
            // parent↔blocks cycle — the standard backfill/gate case — is
            // legal) never land in the store.
            let d = ctx.store.link_task_dependency(task.id, depends_on.id, kind, &ctx.actor, ctx.now())?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&d)?);
            } else {
                outln!("task {} —{}→ task {} recorded ({} in {}, by {}, {})",
                    short(&task.id), kind.as_str(), short(&depends_on.id), depends_on.state.as_str(), depends_on.title, d.actor, d.created_at.format("%Y-%m-%d %H:%M:%S"));
            }
        }
        TaskCmd::Dependencies { id } => {
            let v = ctx.api().task_dependencies(&id).map_err(render_api_error)?;
            let t = v.task;
            let outgoing = v.outgoing;
            let incoming = v.incoming;
            if json {
                outln!("{}", serde_json::json!({"task": t.id.to_string(), "outgoing": outgoing, "incoming": incoming}));
            } else {
                print_task(&t);
                outln!("---");
                outln!("depends on (outgoing): {}", outgoing.len());
                for d in &outgoing {
                    outln!("  {:<7} {}  {:<13} {}  by {}", d.kind.as_str(), short(&d.depends_on_id), d.depends_on_state.as_str(), d.depends_on_title, d.actor);
                }
                outln!("dependents (incoming): {}", incoming.len());
                for d in &incoming {
                    outln!("  {:<7} {}  {:<13} {}  by {}", d.kind.as_str(), short(&d.task_id), d.task_state.as_str(), d.task_title, d.actor);
                }
                // REQ-DEP-06 + RATIFIED DECISION OQ8: no epic entity exists —
                // an epic VERTEX is a task with no outgoing ACTIVE 'parent'
                // edge and at least one incoming one — named by this same
                // dependency read; `task epics` lists the whole registry.
                let epic_children = incoming.iter().filter(|d| d.kind == DependencyKind::Parent).count();
                let own_parents = outgoing.iter().filter(|d| d.kind == DependencyKind::Parent).count();
                if epic_children > 0 && own_parents == 0 {
                    outln!("epic role: {epic_children} task(s) carry a 'parent' edge into this vertex, the vertex has no parent of its own (the registry is this view, not a table)");
                }
            }
        }
        TaskCmd::Unlink { id, on, kind } => {
            // RATIFIED DECISION OQ7: dependency edges are IMMUTABLE —
            // "removal" is a SUPERSEDED marker. The edge row keeps author
            // and created_at, gains who/when superseded it, and stays in
            // the table forever; only the live graph stops seeing it.
            let kind = DependencyKind::parse(&kind).map_err(anyhow::Error::new)?;
            let task = ctx.store.resolve_task(&id)?;
            let depends_on = ctx.store.resolve_task(&on)?;
            let d = ctx.store.supersede_task_dependency(task.id, depends_on.id, kind, &ctx.actor, ctx.now())?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&d)?);
            } else {
                outln!("task {} —{}→ task {} superseded (edge {} stays as history, by {})",
                    short(&task.id), kind.as_str(), short(&depends_on.id), short(&d.id), ctx.actor);
            }
        }
        TaskCmd::Override { id, blocker, rationale } => {
            // RATIFIED DECISION OQ2 — the human-escape of the dependency
            // gate. The refusal of resume/acceptance is unconditional, the
            // ONE release is this explicit operator ratification ("continue
            // despite"), recorded with the same decisions mechanism every
            // other human gate act uses. `refs` = the FULL task ids it
            // covers, so a NEW blocker opened later is NOT covered by an
            // old decision; closing the last blocker never resumes the task
            // by itself (auto-unpause forbidden) — resume stays a human act.
            let task = ctx.store.resolve_task(&id)?;
            let mut blockers: Vec<String> = Vec::new();
            for b in &blocker {
                blockers.push(ctx.store.resolve_task(b)?.id.to_string());
            }
            if blockers.is_empty() {
                // Without explicit --blocker, ratify against exactly the
                // blockers OPEN right now (computed from stored data).
                let deps = ctx.store.task_dependencies(task.id)?;
                blockers = workflow::open_blockers(&task, &deps);
                if blockers.is_empty() {
                    bail!("task {} has no open blocking dependencies — nothing to override", short(&task.id));
                }
            }
            ctx.store.record_decision(&Decision {
                id: new_id(),
                task_id: task.id,
                kind: decision_kind::DEPENDENCY_OVERRIDE.into(),
                verdict: Verdict::Approved,
                actor: ctx.actor.clone(),
                rationale: rationale.clone(),
                refs: blockers.clone(),
                created_at: ctx.now(),
            })?;
            ctx.store.insert_event(
                Some(task.id),
                event_kind::TASK_DEPENDENCY_OVERRIDDEN,
                &ctx.actor,
                serde_json::json!({ "blockers": blockers }),
            )?;
            if json {
                outln!("{}", serde_json::json!({"task": task.id.to_string(), "override_blockers": blockers}));
            } else {
                outln!("override recorded for task {}: continuing despite {} — the gate stands for any NEW blocker",
                    short(&task.id), blockers.join(", "));
                outln!("the task stays in its hold state until a human runs `devai task resume {}`", short(&task.id));
            }
        }
        TaskCmd::Epics => {
            // REQ-DEP-06 + RATIFIED DECISION OQ8: the epic registry is a
            // VIEW over the same stored graph — epic = a task vertex with no
            // outgoing ACTIVE 'parent' edge and at least one incoming one,
            // derived by one dependency-table query. No epic entity exists.
            // (API task.epics read.)
            let epics = ctx.api().task_epics().map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&epics)?);
            } else {
                for e in &epics {
                    outln!("{}  {:<13} {}  ({} derived child/ren via 'parent')",
                        short(&e.task.id), e.task.state.as_str(), e.task.title, e.derived_children);
                }
                if epics.is_empty() {
                    outln!("(no epic vertices — no 'parent' tree roots in the graph)");
                }
            }
        }
        TaskCmd::Graph => {
            // REQ-DEP-04/05: the planning-level "what is happening" is built
            // purely from the stored graph in ONE query — node = task +
            // state, edge = typed dependency; readiness needs no external
            // information. The epic registry (OQ8) is the same data shaped
            // by `task epics`; the READY mark below is blocker-purity (an
            // operator override releases the GATE, not the readiness view —
            // the blockers stay open until the blocker is ACCEPTED, OQ1).
            // (API task.graph read.)
            let gv = ctx.api().task_graph().map_err(render_api_error)?;
            let nodes = gv.graph;
            if json {
                outln!("{}", serde_json::json!({"graph": nodes, "ready": gv.ready}));
            } else {
                for n in &nodes {
                    let blockers = workflow::open_blockers(&n.task, &n.dependencies);
                    let mark = if n.task.state.is_terminal() {
                        "terminal".to_string()
                    } else if blockers.is_empty() {
                        "ready".to_string()
                    } else {
                        format!("open blockers: {}", blockers.join(", "))
                    };
                    outln!("{}  {:<13} {}  [{mark}]", short(&n.task.id), n.task.state.as_str(), n.task.title);
                    for d in &n.dependencies {
                        outln!("    —{:<7}→ {}  {:<13} {}", d.kind.as_str(), short(&d.depends_on_id), d.depends_on_state.as_str(), d.depends_on_title);
                    }
                }
                if nodes.is_empty() {
                    outln!("(no tasks)");
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
    // REQ-SW-05: while the task carries an unresolved RUNNING execution —
    // live writer or orphan — `task run` refuses with the blocking row's
    // full id and the allowed exits. The engine's own start would hit the
    // same wall at the index (enforcement point), this is the labeled,
    // early refusal. A pre-check failure on an unknown id resolves inside
    // the engine exactly as before.
    if let Ok(task) = ctx.store.resolve_task(id) {
        running_writer_guard(ctx, &task)?;
    }
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
    // (API task.report read — QualityReportView is the machine-complete
    // form; REQ-005 CURRENT-by-status selection lives in the API layer.)
    let v = ctx.api().task_report(id).map_err(render_api_error)?;
    if json {
        outln!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    outln!(
        "quality report v{}  artifact {}  contract {}  execution {}",
        v.seq,
        short_str(&v.artifact),
        v.contract,
        v.execution.as_deref().map(short_str).unwrap_or_else(|| "-".to_string())
    );
    outln!("---");
    match v.report_text {
        Some(text) => outln!("{text}"),
        // Reports assembled before @2 carry no rendering: show the machine
        // JSON rather than pretend there is nothing to read.
        None => outln!("{}", serde_json::to_string_pretty(&v.report).unwrap_or_default()),
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
    // (API task.status read — StatusView is the machine-complete form.)
    let v = ctx.api().task_status(id).map_err(render_api_error)?;
    let t = v.task;
    let ops: Vec<String> = v.allowed_operations;
    let current: Vec<Artifact> = v.current_artifacts;
    let open_q: Vec<Question> = v.open_questions;
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

/// `devai task progress <ID>` — pipeline position at a glance. The view data
/// comes from the API task.progress operation (the machine-complete
/// ProgressView); the renderer owns no state-machine knowledge of its own:
/// the spine order, position/re-entry and the running-execution snapshot are
/// computed in the API layer from `workflow::pipeline_*` / `allowed_ops` /
/// `store.list_executions`, change the engine's op set and the view follows.
fn progress(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    let v = ctx.api().task_progress(id).map_err(render_api_error)?;

    if json {
        outln!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    let t = &v.task;
    let ops = &v.allowed_operations;
    let order = &v.pipeline.order;
    let pos_idx = v.pipeline.position_index;
    let running: Vec<_> = v.running_executions.iter().collect();

    outln!("id:    {}", t.id);
    outln!("title: {}", t.title);
    outln!(
        "state: {}{}",
        t.state.as_str(),
        t.return_state.map(|r| format!(" (returns to {})", r.as_str())).unwrap_or_default()
    );
    let mut nodes = Vec::with_capacity(order.len());
    for (i, name) in order.iter().enumerate() {
        if Some(i) == pos_idx && !t.state.is_terminal() {
            nodes.push(format!("[{name}]"));
        } else if pos_idx.is_some_and(|p| p > i) {
            nodes.push(format!("✓ {name}"));
        } else {
            nodes.push(name.clone());
        }
    }
    outln!("pipeline: {}", nodes.join(" → "));
    if t.state.is_terminal() {
        outln!("cycle ended in {} — terminal state, no open pipeline step", t.state.as_str());
    } else if v.pipeline.off_spine {
        // REWORK loop / hold state: the bracket marks where it re-enters.
        match v.pipeline.position.as_deref() {
            Some(p) => outln!("note:  {} is off the spine — it returns to [{}]", t.state.as_str(), p),
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
                short_str(&e.id),
                human_elapsed(e.elapsed_seconds)
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
                Some(e) => {
                    // Ratified OQ-4: a publication into a row that is no
                    // longer RUNNING is a LOUD refusal — the orphan whose
                    // process survived its own cancellation must not write
                    // into a closed execution («зомби не пишет в закрытую
                    // строку»). Work that continues needs a NEW execution
                    // opened by the platform (and the single-writer index
                    // will let only one exist).
                    let ex = ctx.store.get_execution(e)?;
                    if ex.status != ExecutionStatus::Running {
                        bail!("execution {} is {}, not RUNNING — artifact publication requires the producing execution to be RUNNING (a closed row stays closed: work that continues starts a new execution)", ex.id, ex.status.as_str());
                    }
                    Some(ex.id)
                }
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
            // (API artifact.list read.)
            let as_ = ctx.api().artifact_list(&id).map_err(render_api_error)?;
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
            // (API artifact.bytes read — the raw stored content, identical
            // bytes to the pre-API path; the central raw-bytes path stays a
            // closed reader dies quietly (REQ-PIPE-01).)
            let bytes = ctx.api().artifact_bytes(&artifact_id).map_err(render_api_error)?;
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
        let schema = ctx.store.get_contract(&c.name, c.version)?;
        if schema.is_none() {
            bail!("contract {c} is not registered (devai contract add)");
        }
        // Gate parity (ratified OQ-FG-05, REQ-FG-08): a manual
        // `devai artifact publish` runs the SAME contract gate as the engine
        // loop — otherwise a non-contract-shaped body enters the store via
        // the CLI and a later stage re-validates on the role's behalf. The
        // decision ratified the PRESENTATION (warning + the FULL violation
        // list) and the scope; the spec fixes enforcement
        // ("контракт-гейт не обходится выбором пути публикации", AC: the
        // manual path rejects with the same list) — so: print every
        // violation, then refuse.
        let violations = devai_runtime::contract_violations(String::from_utf8_lossy(&content).as_ref(), schema.as_deref())?;
        if !violations.is_empty() {
            for v in &violations {
                eprintln!("warning: {c} contract gate rejected this artifact: {v}");
            }
            bail!("artifact does not satisfy contract {c}: {violations:?}");
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
            // (API question.list read.)
            let qs = ctx.api().question_list(&id).map_err(render_api_error)?;
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
            // (API role.list read.)
            let rs = ctx.api().role_list().map_err(render_api_error)?;
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
            // (API agent.list read.)
            let as_ = ctx.api().agent_list().map_err(render_api_error)?;
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
        ContractCmd::Get { name, version } => match ctx.api().contract_get(&name, version).map_err(render_api_error)? {
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

/// REQ-SW-02/05: the CLI refusal shown BEFORE an attempt is made, when the
/// task already carries a RUNNING execution. This is the labeled
/// application-level check; the ENFORCEMENT point stays the SQLite partial
/// unique index (spec A-5 — a pre-check never replaces the index, and a
/// race between this read and the INSERT still lands on
/// `StoreError::SingleWriterBusy`). The refusal is distinct per ratified
/// liveness class (OQ-3/OQ-5): a LIVE writer means the one-writer invariant
/// fired; a DEAD owner is an orphan only a human resolves; an owner that
/// cannot be checked renders as-is, never as dead. The only resolution is
/// closing the row with `exec finish|fail|cancel` — the process itself is
/// never touched (REQ-SW-06).
fn running_writer_guard(ctx: &Ctx, task: &Task) -> Result<()> {
    let blocker = ctx.store.list_executions(Some(task.id))?.into_iter().find(|e| e.status == ExecutionStatus::Running);
    let Some(e) = blocker else { return Ok(()) };
    let owner = match (e.owner_pid, e.owner_host.as_deref()) {
        (Some(pid), Some(host)) => format!("pid {pid}@{host}"),
        (Some(pid), None) => format!("pid {pid} (host unrecorded)"),
        (None, _) => "owner unrecorded (row predates the v7 guard)".to_string(),
    };
    let verdict = match liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start) {
        liveness::Liveness::Alive => format!("a LIVE writer is in flight ({owner}) — wait for it to finish, or stop it explicitly"),
        liveness::Liveness::Dead => format!("an ORPHAN ({owner} is no longer running) — nobody is collecting this writer's work"),
        liveness::Liveness::Unknown => format!("a writer whose owner cannot be verified ({owner}) — never assumed dead"),
    };
    bail!(
        "single-writer guard: task `{}` already has a RUNNING execution `{}` — {} ({}). \
         The second writer is refused by design: no queueing, no overwrite. \
         Resolve the open row first (a human decision, the process is not touched): \
         `devai exec finish|fail|cancel {}`",
        task.id, e.id, verdict, short(&e.id), e.id
    )
}

fn exec(ctx: &Ctx, cmd: ExecCmd, json: bool) -> Result<()> {
    match cmd {
        ExecCmd::Start { id, role, agent, inputs } => {
            let task = ctx.store.resolve_task(&id)?;
            workflow::check_execution_allowed(&role, &task).map_err(anyhow::Error::new)?;
            // REQ-SW-02 AC2 (CLI path): refuse the second RUNNING row with
            // the blocking id named, before the index would reject it.
            running_writer_guard(ctx, &task)?;
            let rd = ctx.store.get_role(&role)?;
            let version = rd.as_ref().map(|r| r.version).unwrap_or(1);
            let inputs_str: Vec<String> = resolve_ids(ctx, &inputs)?.iter().map(|i| i.to_string()).collect();
            // REQ-001/REQ-006: even a manually started execution binds to the
            // workspace state the platform can observe right now (cwd),
            // computed platform-side — never supplied by the caller.
            let revision = Engine::new(&ctx.store).current_revision().map_err(anyhow::Error::new)?;
            // REQ-SW-03: the owner columns are captured platform-side, the
            // same way as the revision — this CLI process is the one
            // OPENING the row. When it exits and the row is still RUNNING,
            // the orphan detect (REQ-SW-04) will find exactly that.
            let (owner_pid, owner_host, owner_process_start) = liveness::current_owner().into_columns();
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
                // A manual start is its own root attempt (REQ-FG-06).
                attempt: 1,
                parent_execution: None,
                failure_class: None,
                owner_pid,
                owner_host,
                owner_process_start,
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
            // (API exec.list read.)
            let es = ctx.api().exec_list(task.as_deref()).map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&es)?);
            } else if es.is_empty() {
                outln!("(no executions)");
            } else {
                for e in es {
                    // The form loop is readable from ROW DATA (REQ-FG-07):
                    // attempt number and failure class are stored columns,
                    // not something the display reconstructs by parsing text.
                    let mut flags = String::new();
                    if e.attempt > 1 {
                        flags.push_str(&format!(" attempt#{}", e.attempt));
                    }
                    if let Some(fc) = &e.failure_class {
                        flags.push_str(&format!(" [{fc}]"));
                    }
                    outln!("{}  {:<22} {:<12} {:<10}{} {}", short(&e.id), e.role_slug, short(&e.task_id), e.status.as_str(), flags,
                        e.summary.as_deref().or(e.error.as_deref()).map(|s| ellipsis(s, 60)).unwrap_or_default());
                }
            }
        }
        ExecCmd::Get { exec_id } => {
            // (API exec.get read: one execution row by id/prefix — a second
            // consumer sees the same row the launching CLI saw, REQ-API-07 AC1.)
            let e = ctx.api().exec_get(&exec_id).map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&e)?);
            } else {
                outln!("id:        {}", e.id);
                outln!("task:      {}", e.task_id);
                outln!("role:      {}@{}", e.role_slug, e.role_version);
                outln!("agent:     {}", e.agent_name.as_deref().unwrap_or("-"));
                outln!("status:    {}", e.status.as_str());
                outln!("attempt:   {}", e.attempt);
                outln!("started:   {}", e.started_at.format("%Y-%m-%d %H:%M:%S"));
                if let Some(f) = e.finished_at {
                    outln!("finished:  {}", f.format("%Y-%m-%d %H:%M:%S"));
                }
                if let Some(s) = &e.summary {
                    outln!("summary:   {}", s);
                }
                if let Some(err) = &e.error {
                    outln!("error:     {}", err);
                }
                if let Some(fc) = &e.failure_class {
                    outln!("failure:   [{fc}]");
                }
            }
        }
        ExecCmd::Liveness { exec_id } => {
            // (API exec.liveness read.) Read-only triage of the recorded
            // owner: alive | dead | unknown — an orphan RUNNING row is
            // distinguishable by stored data, not operator pgrep
            // (REQ-API-07 AC3). Never signals the process (REQ-SW-06).
            let v = ctx.api().exec_liveness(&exec_id).map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                outln!("execution {}  status {}  owner pid={:?} host={:?}  liveness: {}",
                    v.id, v.status, v.owner_pid, v.owner_host, v.liveness);
            }
        }
    }
    Ok(())
}

/// `devai task active [--json]` — «что сейчас активно» in one view: a
/// read-only snapshot of every RUNNING execution in this store (REQ-ACT-01).
/// OQ-1 as ratified: a separate `task` subcommand, not a flag on `exec list`
/// — the listing of execution rows and the view of running work stay apart.
/// Like the progress view, the renderer owns no state-machine knowledge: the
/// stage is the task's live `TaskState::as_str()`, resolved through
/// `store.resolve_task` per row (REQ-ACT-02/05), and the worktree link is
/// printed only where the platform recorded the `worktree.prepared` fact for
/// that task — otherwise a dash, never an invented directory. Rows come in
/// elapsed-descending order, oldest work first, ties by id — deterministic
/// (OQ-3 as ratified). The `stale?` mark is the guard task's shipped
/// detect (REQ-SW-04, ratified OQ-5: ADDITIVE, never a replacement): a row
/// is stale where the task is already in a terminal state while its
/// execution is still RUNNING — the store-proven contradiction the view
/// always marked — OR where the row's recorded owner process is provably
/// DEAD on this host (pid gone, or pid recycled under the OQ-3 start-time
/// check). An owner that cannot be checked (other host, no /proc, a row
/// predating rung 007) is NOT dead: it renders `owner?` as a separate
/// triage marker and `owner_liveness: "unknown"` in json, as-is. Detection
/// is READ-ONLY (REQ-SW-06): this view writes nothing and kills no one —
/// resolution belongs to a human via `exec fail|cancel`. The command
/// writes nothing and waits for nothing: print the snapshot, exit 0 — an
/// empty list is the normal idle state, not an error (REQ-ACT-04). It
/// deliberately stops short of the per-task pipeline: `task progress <id>`
/// stays the single owner of that detail, so the row carries the short task
/// id and no title (REQ-ACT-06; OQ-6 as ratified).
fn active_overview(ctx: &Ctx, json: bool) -> Result<()> {
    // (API task.active read — ActiveSnapshot is the machine-complete form:
    // REQ-ACT-01 AC1 this store, all tasks; OQ-3 ordering and the one
    // liveness observation per row live in the API layer. Read-only:
    // REQ-SW-06 — detection never writes and never signals a process.)
    let snap = ctx.api().task_active().map_err(render_api_error)?;

    if json {
        // REQ-ACT-03 / OQ-4 (ratified): exactly one JSON object on stdout —
        // snapshot_at and a flat list of RUNNING rows, nothing before or
        // after: early return, the progress-view precedent.
        outln!("{}", serde_json::to_string_pretty(&snap)?);
        return Ok(());
    }
    if snap.rows.is_empty() {
        outln!("(no running executions)");
        return Ok(());
    }
    for e in &snap.rows {
        // OQ-6 (ratified): the row carries short task id, role@version,
        // elapsed and stage — never the title (it is one `task progress <id>`
        // away); the worktree link only on the platform fact. The trailing
        // `stale?` is the REQ-SW-04 additive mark: terminal task (the mark
        // this view always had) OR provably-dead owner (orphan). A row whose
        // owner cannot be checked gets the separate `owner?` triage mark —
        // never `stale?`, never assumed dead (OQ-3).
        let link = match (&e.branch, &e.worktree) {
            (Some(b), Some(w)) => format!("{b}  {w}"),
            _ => "—".to_string(),
        };
        let stale = if e.stale {
            "  stale?"
        } else if e.owner_liveness == "unknown" {
            "  owner?"
        } else {
            ""
        };
        outln!(
            "  {:<12} {:<24} {:<16} {:<13} {}{}",
            short_str(&e.task_id),
            format!("{}@{}", e.role_slug, e.role_version),
            e.state,
            format!("elapsed {}", human_elapsed(e.elapsed_seconds)),
            link,
            stale,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// export / import
// ---------------------------------------------------------------------------

fn export(ctx: &Ctx, id: &str, out: &Path, json: bool) -> Result<()> {
    // (API export.bundle — the same bundle path over the same store:
    // identical data and artifact version identity via the API, REQ-API-02 AC3.)
    let manifest = ctx.api().export_bundle(id, out).map_err(render_api_error)?;
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
    // The canonical width + rationale live in devai_core::types::ID_PREFIX_CHARS
    // (single source, shared with devai-api — R1 QA no-magic).
    devai_core::types::short_id(id)
}

/// Display-short of an id that already arrives as a string (API views).
pub(crate) fn short_str(id: &str) -> String {
    devai_core::types::short_id_str(id)
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
