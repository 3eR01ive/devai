//! The CLI surface. Cycle 2 (REQ-MUT-02/04): the command layer owns NO
//! store handle and touches NO store — every read AND every write runs as
//! a typed operation of `devai_api::Api`, and the handlers below only
//! render results (workflow rules stay where the API delegates:
//! `core::workflow` + the store, unchanged). Bootstrap is encapsulated too
//! (human-ratified Q2): `Api::open` owns the home, `Api::seed_config`
//! seeds it; `init`/seed are not registry operations, they are the
//! ratified OQ-10 bootstrap pair behind the API surface.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use devai_core::types::*;
use devai_core::workflow;
use devai_runtime::{Gate, worktree};
use uuid::Uuid;

use crate::outln;
use crate::{
    AgentCmd, ApiCmd, ArtifactCmd, Cmd, ContractCmd, ExecCmd, ProjectCmd, QuestionCmd, RoleCmd,
    TaskCmd,
};
use devai_api::{
    AdvanceReq, AgentAddReq, Api, ApiError, AskReq, ContractAddReq, DependReq, ExecStartReq,
    MergeRecordReq, MutationResult, ProjectAddReq, PublishReq, ReviseReq, RunReq, TaskCreateReq,
};

pub struct Ctx {
    api: Api,
}

impl Ctx {
    pub fn new(home: Option<&Path>, actor: Option<String>) -> Result<Ctx> {
        let dir = match home {
            Some(p) => p.to_path_buf(),
            None => {
                let base =
                    dirs::home_dir().context("no home directory; pass --home or set DEVAI_HOME")?;
                base.join(".devai")
            }
        };
        let actor =
            actor.unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "user".into()));
        // Cycle 2 (REQ-MUT-04, human-ratified Q2): the API opens the home
        // and OWNS the store handle — the bootstrap entry stays outside the
        // operation registry (RATIFIED OQ-10) but is no longer consumer-
        // side store access. Every command dispatches through `Ctx::api`.
        let api = Api::open(&dir, &actor).context("open local store")?;
        Ok(Ctx { api })
    }

    /// The platform API surface this CLI consumes as its first consumer
    /// (REQ-API-03, ratified OQ-1: in-process crate). After cycle 2 this
    /// is the ONLY path to data: reads (REQ-API-03 AC1) and mutations
    /// (REQ-MUT-02) alike — no direct store handle exists in this crate
    /// (pinned by tests/api_cycle2_mutations.rs::cli_sources_touch_no_store).
    pub fn api(&self) -> &Api {
        &self.api
    }
}

/// Render an API error the way the CLI rendered the store error before the
/// API layer existed: `devai: <message>` on stderr, nonzero exit
/// (REQ-API-04 AC2 — an API failure is an explicit message, never an empty
/// or truncated output). `ApiError` Display forwards the inner text, so the
/// human-visible message is byte-identical to the pre-API path; mutation
/// refusals carry the operator text with a machine-readable code beside it
/// (REQ-MUT-08).
pub fn render_api_error(e: devai_api::ApiError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

pub fn run(ctx: &Ctx, cmd: Cmd, json: bool) -> Result<()> {
    match cmd {
        // RATIFIED OQ-10 + human-ratified cycle-2 Q2: `init` seeds through
        // `Api::seed_config` — the bootstrap pair is encapsulated behind
        // the API (no consumer-side Store) yet never an operation of the
        // registry.
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
                outln!(
                    "--- {} operations published (RATIFIED OQ-10 init/seed stay the bootstrap pair encapsulated behind the API, not operations of this registry)",
                    ops.len()
                );
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
        Some(dir) => (
            ctx.api().seed_config(Some(dir)).map_err(render_api_error)?,
            dir.to_path_buf(),
        ),
        None => {
            let dir = ctx.api().home().join("config");
            (ctx.api().seed_config(None).map_err(render_api_error)?, dir)
        }
    };
    if json {
        outln!(
            "{}",
            serde_json::json!({"home": ctx.api().home(), "config": cfg_path, "roles_seeded": seeded})
        );
    } else {
        outln!("devai home: {}", ctx.api().home().display());
        outln!("config tree: {}", cfg_path.display());
        if seeded > 0 {
            outln!("seeded/updated {seeded} role version(s) from config");
            outln!(
                "edit {}/criteria/*.yaml, evaluation/*.yaml or roles/*.yaml and re-run `devai init` to bump versions",
                cfg_path.display()
            );
            if config_dir.is_some() {
                outln!(
                    "the engine reads evaluation profiles from $DEVAI_CONFIG — export DEVAI_CONFIG={}",
                    cfg_path.display()
                );
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
        ProjectCmd::Add {
            name,
            description,
            repo,
        } => {
            let p = ctx
                .api()
                .project_add(ProjectAddReq {
                    name,
                    description,
                    repos: repo,
                })
                .map_err(render_api_error)?;
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

/// Cycle 2 (REQ-MUT-02): every mutation arm below calls an `Api` operation;
/// the handler owns only presentation. Reads were routed the same way since
/// cycle 1 — the CYCLE-2 PENDING marks are gone because the plane moved.
fn task(ctx: &Ctx, cmd: TaskCmd, json: bool) -> Result<()> {
    match cmd {
        TaskCmd::New {
            title,
            project,
            description,
            description_file,
            parent,
            blocked_by,
        } => {
            // The project name resolves through the API read FIRST, so its
            // not-found refusal keeps the exact position the command had.
            let project_id = match project.as_deref() {
                Some(name) => Some(
                    ctx.api()
                        .project_get(name)
                        .map_err(render_api_error)?
                        .context(format!("project `{name}` not found"))?
                        .id,
                ),
                None => None,
            };
            let desc = match description_file {
                Some(f) => {
                    std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?
                }
                None => description,
            };
            // REQ-DEP-07: a derived task records WHY it exists at the moment
            // it is created — the genealogy/blocking never leaves for the
            // operator chat. Dangling references fail loudly in the
            // operation, exactly as they would via `task depend`.
            let created = ctx
                .api()
                .task_create(TaskCreateReq {
                    title,
                    project_id,
                    description: desc,
                    parent,
                    blocked_by,
                })
                .map_err(render_api_error)?;
            let t = created.task;
            let linked = created.linked;
            if json {
                outln!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                outln!(
                    "task {} created in DRAFT — workflow is owned by devai",
                    short(&t.id)
                );
                for d in &linked {
                    outln!(
                        "  depends {} {} ({} in {})",
                        d.kind.as_str(),
                        short(&d.depends_on_id),
                        d.depends_on_state.as_str(),
                        d.depends_on_title
                    );
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
                    outln!(
                        "{}  {:<17}  v{:>2}  {}",
                        short(&t.id),
                        t.state.as_str(),
                        t.requirements_seq,
                        t.title
                    );
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
        TaskCmd::Start { id } => {
            render_mutation(&ctx.api().task_start(&id).map_err(render_api_error)?)?
        }
        TaskCmd::Run {
            id,
            agent,
            workspace,
            force_reaudit,
        } => task_run(ctx, &id, &agent, workspace.as_deref(), force_reaudit, json)?,
        TaskCmd::Revise { id, file, reason } => {
            let task_ref = ctx.api().task_get(&id).map_err(render_api_error)?;
            let content =
                std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            let o = ctx
                .api()
                .task_revise(ReviseReq {
                    id: task_ref.id.to_string(),
                    name: file.display().to_string(),
                    content,
                    reason,
                })
                .map_err(render_api_error)?;
            outln!(
                "requirements v{} published → REVIEW",
                o.task.requirements_seq
            );
            if !o.staled_ids.is_empty() {
                outln!(
                    "consequences: {} artifact(s) marked STALE:",
                    o.staled_ids.len()
                );
                for s in &o.staled_ids {
                    outln!("  stale  {s}");
                }
                outln!("old versions are kept in history; nothing was deleted");
            }
            outln!("next: {}", o.allowed_operations.join(" | "));
        }
        TaskCmd::Review {
            id,
            verdict,
            summary,
        } => {
            let v = Api::parse_verdict(&verdict).map_err(render_api_error)?;
            render_mutation(
                &ctx.api()
                    .task_review(&id, v, summary)
                    .map_err(render_api_error)?,
            )?;
        }
        TaskCmd::Approve { id, rationale } => {
            // The approve act is persisted like every other human-gate act:
            // a workflow event AND a stored requirements_approval decision
            // row — the row is what makes the rationale a fact the QA
            // auditor is fed (precedent 01a1a0b5-0fcf). The API operation
            // owns both writes in the old order.
            let o = ctx
                .api()
                .task_approve(&id, rationale)
                .map_err(render_api_error)?;
            render_mutation(&o.mutation)?;
        }
        TaskCmd::Reject { id, rationale } => render_mutation(
            &ctx.api()
                .task_reject(&id, rationale)
                .map_err(render_api_error)?,
        )?,
        TaskCmd::Accept { id, rationale } => {
            // REQ-DEP-03: the human acceptance gate reads the stored
            // dependency graph — an open 'blocks' edge refuses the gate and
            // the refusal enumerates the blocking task ids as data. The one
            // release (RATIFIED DECISION OQ2) is the operator's explicit
            // `dependency_override` decision (see `task override` below).
            render_mutation(
                &ctx.api()
                    .task_accept(&id, rationale)
                    .map_err(render_api_error)?,
            )?;
        }
        TaskCmd::Return { id, rationale } => render_mutation(
            &ctx.api()
                .task_return(&id, rationale)
                .map_err(render_api_error)?,
        )?,
        TaskCmd::RatifyEmptyRework { id, rationale } => {
            ratify_empty_rework(ctx, &id, rationale, json)?
        }
        TaskCmd::Merge {
            id,
            workspace,
            rationale,
        } => task_merge(ctx, &id, workspace.as_deref(), rationale, json)?,
        TaskCmd::Pause { id, reason } => render_mutation(
            &ctx.api()
                .task_pause(&id, reason)
                .map_err(render_api_error)?,
        )?,
        TaskCmd::Resume { id } => {
            // REQ-DEP-03: resume from a hold state consults the stored graph
            // (open blockers enumerate into the refusal; "closed" = blocker
            // ACCEPTED per ratified OQ1). The only escape is the operator's
            // own `task override` ratification (ratified OQ2). Closing the
            // last blocker does NOT resume the task by itself — auto-unpause
            // is forbidden, resume stays a human command.
            render_mutation(&ctx.api().task_resume(&id).map_err(render_api_error)?)?;
        }
        TaskCmd::Block { id, reason } => render_mutation(
            &ctx.api()
                .task_block(&id, reason)
                .map_err(render_api_error)?,
        )?,
        TaskCmd::Cancel { id, reason } => render_mutation(
            &ctx.api()
                .task_cancel(&id, reason)
                .map_err(render_api_error)?,
        )?,
        TaskCmd::Fail { id, reason } => {
            render_mutation(&ctx.api().task_fail(&id, reason).map_err(render_api_error)?)?
        }
        TaskCmd::Events { id, limit } => {
            let evs = ctx
                .api()
                .task_events(&id, limit)
                .map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&evs)?);
            } else {
                for e in evs {
                    outln!(
                        "{}  {:<28} {:<12} {}",
                        e.created_at.format("%Y-%m-%d %H:%M:%S"),
                        e.kind,
                        e.actor,
                        compact(&e.payload)
                    );
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
                    outln!(
                        "{}  {:<26} {:<10} by {}  {}",
                        d.created_at.format("%Y-%m-%d %H:%M:%S"),
                        d.kind,
                        d.verdict.as_str(),
                        d.actor,
                        d.rationale.map(|s| ellipsis(&s, 100)).unwrap_or_default()
                    );
                    if !d.refs.is_empty() {
                        outln!("    refs: {}", d.refs.join(", "));
                    }
                }
            }
        }
        TaskCmd::Depend { id, on, kind } => {
            let kind = DependencyKind::parse(&kind).map_err(anyhow::Error::new)?;
            // Registration is LOUD at write (REQ-DEP-07 AC2): self-reference,
            // a duplicate edge, a dangling id or a cycle WITHIN one kind
            // (ratified OQ4: parent-DAG and blocks-DAG; a mixed
            // parent↔blocks cycle — the standard backfill/gate case — is
            // legal) never land in the store.
            let d = ctx
                .api()
                .task_depend(DependReq { id, on, kind })
                .map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&d)?);
            } else {
                outln!(
                    "task {} —{}→ task {} recorded ({} in {}, by {}, {})",
                    short(&d.task_id),
                    d.kind.as_str(),
                    short(&d.depends_on_id),
                    d.depends_on_state.as_str(),
                    d.depends_on_title,
                    d.actor,
                    d.created_at.format("%Y-%m-%d %H:%M:%S")
                );
            }
        }
        TaskCmd::Dependencies { id } => {
            let v = ctx.api().task_dependencies(&id).map_err(render_api_error)?;
            let t = v.task;
            let outgoing = v.outgoing;
            let incoming = v.incoming;
            if json {
                outln!(
                    "{}",
                    serde_json::json!({"task": t.id.to_string(), "outgoing": outgoing, "incoming": incoming})
                );
            } else {
                print_task(&t);
                outln!("---");
                outln!("depends on (outgoing): {}", outgoing.len());
                for d in &outgoing {
                    outln!(
                        "  {:<7} {}  {:<13} {}  by {}",
                        d.kind.as_str(),
                        short(&d.depends_on_id),
                        d.depends_on_state.as_str(),
                        d.depends_on_title,
                        d.actor
                    );
                }
                outln!("dependents (incoming): {}", incoming.len());
                for d in &incoming {
                    outln!(
                        "  {:<7} {}  {:<13} {}  by {}",
                        d.kind.as_str(),
                        short(&d.task_id),
                        d.task_state.as_str(),
                        d.task_title,
                        d.actor
                    );
                }
                // REQ-DEP-06 + RATIFIED DECISION OQ8: no epic entity exists —
                // an epic VERTEX is a task with no outgoing ACTIVE 'parent'
                // edge and at least one incoming one — named by this same
                // dependency read; `task epics` lists the whole registry.
                let epic_children = incoming
                    .iter()
                    .filter(|d| d.kind == DependencyKind::Parent)
                    .count();
                let own_parents = outgoing
                    .iter()
                    .filter(|d| d.kind == DependencyKind::Parent)
                    .count();
                if epic_children > 0 && own_parents == 0 {
                    outln!(
                        "epic role: {epic_children} task(s) carry a 'parent' edge into this vertex, the vertex has no parent of its own (the registry is this view, not a table)"
                    );
                }
            }
        }
        TaskCmd::Unlink { id, on, kind } => {
            // RATIFIED DECISION OQ7: dependency edges are IMMUTABLE —
            // "removal" is a SUPERSEDED marker. The edge row keeps author
            // and created_at, gains who/when superseded it, and stays in
            // the table forever; only the live graph stops seeing it.
            let kind = DependencyKind::parse(&kind).map_err(anyhow::Error::new)?;
            let u = ctx
                .api()
                .task_unlink(DependReq { id, on, kind })
                .map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&u.edge)?);
            } else {
                outln!(
                    "task {} —{}→ task {} superseded (edge {} stays as history, by {})",
                    short(&u.edge.task_id),
                    kind.as_str(),
                    short(&u.edge.depends_on_id),
                    short(&u.edge.id),
                    u.superseded_by
                );
            }
        }
        TaskCmd::Override {
            id,
            blocker,
            rationale,
        } => {
            // RATIFIED DECISION OQ2 — the human-escape of the dependency
            // gate. The refusal of resume/acceptance is unconditional, the
            // ONE release is this explicit operator ratification ("continue
            // despite"), recorded with the same decisions mechanism every
            // other human gate act uses. `refs` = the FULL task ids it
            // covers, so a NEW blocker opened later is NOT covered by an
            // old decision; closing the last blocker never resumes the task
            // by itself (auto-unpause forbidden) — resume stays a human act.
            let r = ctx
                .api()
                .task_override(&id, blocker, rationale)
                .map_err(render_api_error)?;
            let blockers = r.override_blockers;
            if json {
                outln!(
                    "{}",
                    serde_json::json!({"task": r.task.id.to_string(), "override_blockers": blockers})
                );
            } else {
                outln!(
                    "override recorded for task {}: continuing despite {} — the gate stands for any NEW blocker",
                    short(&r.task.id),
                    blockers.join(", ")
                );
                outln!(
                    "the task stays in its hold state until a human runs `devai task resume {}`",
                    short(&r.task.id)
                );
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
                    outln!(
                        "{}  {:<13} {}  ({} derived child/ren via 'parent')",
                        short(&e.task.id),
                        e.task.state.as_str(),
                        e.task.title,
                        e.derived_children
                    );
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
                    outln!(
                        "{}  {:<13} {}  [{mark}]",
                        short(&n.task.id),
                        n.task.state.as_str(),
                        n.task.title
                    );
                    for d in &n.dependencies {
                        outln!(
                            "    —{:<7}→ {}  {:<13} {}",
                            d.kind.as_str(),
                            short(&d.depends_on_id),
                            d.depends_on_state.as_str(),
                            d.depends_on_title
                        );
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

/// Render the committed workflow mutation exactly as the old CLI `op()`
/// helper printed it: transition line, STALE/SUPERSEDED consequences, the
/// next allowed operations. The facts arrive typed from the API
/// (REQ-MUT-01: the operation returns the workflow result, not a store
/// proxy; REQ-MUT-08: one rendering path for success and refusal).
fn render_mutation(m: &MutationResult) -> Result<()> {
    if m.task.state != m.previous_state {
        outln!(
            "state: {} → {}",
            m.previous_state.as_str(),
            m.task.state.as_str()
        );
    } else {
        outln!("recorded ({})", m.event_kinds[0]);
    }
    if m.staled > 0 {
        outln!(
            "marked STALE: {} dependent artifact(s) — history preserved",
            m.staled
        );
    }
    if m.superseded > 0 {
        outln!("superseded: {}", m.superseded);
    }
    if !m.allowed_operations.is_empty() {
        outln!("next: {}", m.allowed_operations.join(" | "));
    }
    Ok(())
}

/// Drive the task through every AUTONOMOUS stage with a real role
/// execution (API `task.run` — the engine drive including the single-writer
/// guard, which moved into the operation and keeps its labeled refusal
/// text). Stops at human gates (APPROVAL, acceptance) — never past them.
fn task_run(
    ctx: &Ctx,
    id: &str,
    agent: &str,
    workspace: Option<&Path>,
    force_reaudit: bool,
    json: bool,
) -> Result<()> {
    let o = ctx
        .api()
        .task_run(RunReq {
            id: id.to_string(),
            agent: agent.to_string(),
            workspace: workspace.map(|p| p.to_path_buf()),
            force_reaudit,
        })
        .map_err(render_api_error)?;
    let report = o.report;
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
        Gate::WaitingApproval => {
            outln!("⏸ APPROVAL is a human gate — approve or reject the requirements")
        }
        Gate::WaitingAccept => {
            // The §2.6 rendering is for the human at the gate.
            if let Some(text) = &o.gate_report_text {
                outln!("── quality report (platform-assembled) ──");
                outln!("{text}");
            }
            outln!("⏸ acceptance is a human gate — accept or return the result");
        }
        Gate::ReportFresh => {
            // The freshness gate stopped the run: no auditor execution was
            // started, no artifact was published. The human decides.
            // REQ-005: CURRENT-by-status selection lives in the API.
            if let Some(a) = &o.current_quality_report {
                outln!(
                    "⏸ quality report v{} ({}) is FRESH — it is bound to exactly the recomputed workspace revision; the auditor was NOT re-run",
                    a.seq,
                    short(&a.id)
                );
            } else {
                outln!(
                    "⏸ quality report is FRESH for the current workspace revision — the auditor was NOT re-run"
                );
            }
            if let Some(text) = &o.gate_report_text {
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
    if !o.allowed_operations.is_empty() {
        outln!("next: {}", o.allowed_operations.join(" | "));
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
/// act, recorded (API `task.merge`) as a decision + event, not a workflow
/// transition. Cycle 2 moves the RECORDING behind the API; the repository
/// act stays here exactly where REQ-WT-07 ratified it.
fn task_merge(
    ctx: &Ctx,
    id: &str,
    workspace: Option<&Path>,
    rationale: Option<String>,
    json: bool,
) -> Result<()> {
    let task = ctx.api().task_get(id).map_err(render_api_error)?;
    let repo = match workspace {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("no current directory; pass --workspace")?,
    };
    if !worktree::is_git_repo(&repo) {
        bail!(
            "{} is not a git repository — nothing to merge into",
            repo.display()
        );
    }
    let branch = worktree::branch_name(task.id);
    if !worktree::has_branch(&repo, task.id) {
        bail!(
            "task {} has no `{branch}` branch — a writing stage creates it via the platform worktree; there is nothing to merge",
            short(&task.id)
        );
    }
    let head_before = git_in(&repo, &["rev-parse", "--verify", "HEAD"])
        .context("baseline checkout has no commits to merge into")?;
    git_in(&repo, &["merge", "--no-edit", &branch])?;
    let head_after = git_in(&repo, &["rev-parse", "HEAD"])?;

    let r = ctx
        .api()
        .task_merge_record(MergeRecordReq {
            id: task.id.to_string(),
            branch: branch.clone(),
            workspace: repo.display().to_string(),
            head_before: head_before.clone(),
            head_after: head_after.clone(),
            rationale,
        })
        .map_err(render_api_error)?;
    if json {
        outln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "task": r.task_id.to_string(), "branch": r.branch, "workspace": r.workspace,
                "head_before": r.head_before, "head_after": r.head_after,
            }))?
        );
    } else {
        outln!(
            "merged `{branch}` into {}  {head_before} → {head_after}",
            repo.display()
        );
        outln!(
            "branch and worktree persist — their removal is a lifecycle decision the platform does not automate (OQ-1)"
        );
    }
    Ok(())
}

/// The EMPTY-REWORK GATE escape (REQ-RG-03), human-only like the merge gate:
/// the API `task.ratify_empty_rework` operation records (or reuses) the
/// pair-bound `rework_noop_ratified` Decision for the exact report pair
/// named by the most recent `qa.rework_gate_tripped` event of the latest
/// report, releases the gate's blocking question, and runs the withheld
/// REWORK → QA transition through the same core command
/// (`implementation_reported`), which now finds the pair-bound ratification
/// and cites its decision id on the transition event (AC3). No engine or
/// agent path reaches this handler — the decision can only be created by a
/// human invoking the CLI (AC2), and there is no timeout auto-pass (AC
/// constraint): without the explicit act the task simply stays parked.
fn ratify_empty_rework(ctx: &Ctx, id: &str, rationale: Option<String>, json: bool) -> Result<()> {
    let r = ctx
        .api()
        .task_ratify_empty_rework(id, rationale)
        .map_err(render_api_error)?;
    if json {
        outln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "task": r.task.id.to_string(),
                "decision": r.decision.id.to_string(),
                "recorded_now": r.recorded_now,
                "prev_report": r.prev_report,
                "current_report": r.current_report,
                "state": r.state.as_str(),
            }))?
        );
    } else {
        outln!(
            "ratified: decision {} ({}) — the no-op rework is now a conscious operator passage",
            r.decision.id,
            if r.recorded_now {
                "recorded"
            } else {
                "reused stored"
            }
        );
        outln!(
            "pair: report {} → report {} on the same commit",
            r.prev_report,
            r.current_report
        );
        outln!(
            "state: {} → QA; the transition event cites the decision",
            r.task.state.as_str()
        );
        outln!("next: {}", r.allowed_operations.join(" | "));
    }
    Ok(())
}

/// Read-only git plumbing for the merge gate: the CLI never grows domain
/// logic, it only performs exactly the one repository act a human ordered
/// (REQ-WT-07 — the git invocation stays CLI-side BY RATIFICATION; it is a
/// repository act, not store access, and is out of the REQ-MUT-02 scope).
fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("launch git in {}", dir.display()))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
            outln!(
                "  {:<22} v{:<3} {}  {}",
                a.kind.as_str(),
                a.seq,
                short(&a.id),
                a.name
            );
        }
        for q in &open_q {
            outln!(
                "open question {}: {}{}",
                short(&q.id),
                q.text,
                if q.blocking { " [BLOCKING]" } else { "" }
            );
        }
        outln!(
            "allowed operations: {}",
            if ops.is_empty() {
                "(none — task is terminal)".to_string()
            } else {
                ops.join(" | ")
            }
        );
    }
    Ok(())
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
        v.execution
            .as_deref()
            .map(short_str)
            .unwrap_or_else(|| "-".to_string())
    );
    outln!("---");
    match v.report_text {
        Some(text) => outln!("{text}"),
        // Reports assembled before @2 carry no rendering: show the machine
        // JSON rather than pretend there is nothing to read.
        None => outln!(
            "{}",
            serde_json::to_string_pretty(&v.report).unwrap_or_default()
        ),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// artifacts
// ---------------------------------------------------------------------------

fn artifact(ctx: &Ctx, cmd: ArtifactCmd, json: bool) -> Result<()> {
    match cmd {
        ArtifactCmd::Publish {
            id,
            kind,
            file,
            name,
            depends,
            contract,
            execution,
            verdict,
            category,
        } => {
            let kind = ArtifactKind::parse(&kind.replace('-', "_")).map_err(anyhow::Error::new)?;
            let category =
                DataCategory::parse(&category.replace('-', "_")).map_err(anyhow::Error::new)?;
            // The task ref resolves before the file read — the exact order
            // the pre-cycle-2 handler had (error-position parity).
            ctx.api().task_get(&id).map_err(render_api_error)?;
            let content =
                std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            // The publication runs in two API operations so the artifact
            // row prints BEFORE a failing advance — the store effects and
            // their order match the compound `artifact.publish` exactly.
            let row = ctx
                .api()
                .artifact_publish_row(PublishReq {
                    id: id.clone(),
                    kind,
                    name: name.unwrap_or_else(|| file.display().to_string()),
                    content,
                    depends,
                    contract,
                    execution,
                    verdict: verdict.clone(),
                    category,
                })
                .map_err(|e| {
                    // Gate parity (ratified OQ-FG-05, REQ-FG-08): the manual
                    // path runs the SAME contract gate as the engine loop — the
                    // API enforces, the CLI renders the ratified PRESENTATION:
                    // warning + the FULL violation list, then the refusal.
                    if let ApiError::ContractRejected {
                        contract,
                        violations,
                    } = &e
                    {
                        for v in violations {
                            eprintln!(
                                "warning: {contract} contract gate rejected this artifact: {v}"
                            );
                        }
                    }
                    render_api_error(e)
                })?;
            let a = row.artifact;
            if json {
                outln!("{}", serde_json::to_string_pretty(&a)?);
            } else {
                outln!(
                    "artifact {} {} v{} ({})",
                    a.kind.as_str(),
                    short(&a.id),
                    a.seq,
                    a.name
                );
            }
            // Some kinds carry the workflow forward — the advancing set is
            // owned by core (`ArtifactKind::advances_state`, QA R4): this
            // render derives membership from the SAME single source the
            // API compound and the standalone advance use.
            if kind.advances_state() {
                let adv = ctx
                    .api()
                    .artifact_advance(AdvanceReq {
                        id,
                        artifact_id: a.id.to_string(),
                        kind,
                        verdict,
                    })
                    .map_err(render_api_error)?;
                outln!("state: {} → {}", adv.from.as_str(), adv.to.as_str());
                if kind == ArtifactKind::ImplementationReport && adv.rework_gate {
                    outln!(
                        "⏸ EMPTY-REWORK GATE: report {} names the SAME commit as the previous one — the rework captured no new work; QA is withheld.",
                        short(&a.id)
                    );
                    outln!(
                        "   operator: ratify the no-op explicitly with `devai task ratify-empty-rework {} --rationale \"...\"`, or answer the question and re-run the implementation with real fixes",
                        short(&a.task_id)
                    );
                }
            } else if kind == ArtifactKind::QualityReport {
                outln!("quality report recorded; user decides via `devai task accept|return`");
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
                    outln!(
                        "{}  {:<22} v{:<3} {:<12} {}  {}",
                        short(&a.id),
                        a.kind.as_str(),
                        a.seq,
                        a.status.as_str(),
                        a.created_by,
                        a.name
                    );
                }
            }
        }
        ArtifactCmd::Cat { artifact_id } => {
            // (API artifact.bytes read — the raw stored content, identical
            // bytes to the pre-API path; the central raw-bytes path stays a
            // closed reader dies quietly (REQ-PIPE-01).)
            let bytes = ctx
                .api()
                .artifact_bytes(&artifact_id)
                .map_err(render_api_error)?;
            crate::out::write_bytes_all(&bytes)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// questions
// ---------------------------------------------------------------------------

fn question(ctx: &Ctx, cmd: QuestionCmd, json: bool) -> Result<()> {
    match cmd {
        QuestionCmd::Ask {
            id,
            text,
            context,
            options,
            blocking,
            execution,
        } => {
            let asked = ctx
                .api()
                .question_ask(AskReq {
                    id,
                    text,
                    context,
                    options,
                    blocking,
                    execution,
                })
                .map_err(render_api_error)?;
            let q = asked.question;
            if asked.parked {
                outln!(
                    "question {} raised; task parked in WAITING_FOR_INPUT",
                    short(&q.id)
                );
                outln!("answer with: devai question answer {}", short(&q.id));
            } else {
                outln!("question {} recorded", short(&q.id));
            }
        }
        QuestionCmd::Answer { question_id, text } => {
            let o = ctx
                .api()
                .question_answer(&question_id, &text)
                .map_err(render_api_error)?;
            outln!(
                "answer recorded; state: {} → {}",
                o.from_state.as_str(),
                o.to_state.as_str()
            );
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
                    outln!(
                        "{} {:<8} {}{}",
                        short(&q.id),
                        q.status.as_str(),
                        q.text,
                        if q.blocking && q.status == QuestionStatus::Open {
                            " [BLOCKING]"
                        } else {
                            ""
                        }
                    );
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
            let body = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            let r: RoleDefinition =
                serde_json::from_str(&body).context("invalid RoleDefinition JSON")?;
            // The version override (0 = latest+1 for the slug) and the
            // id/timestamp defaulting run inside the API `role.add`.
            let r = ctx.api().role_add(r).map_err(render_api_error)?;
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
                    outln!(
                        "{:<24} v{:<3} {}  in={} out={}",
                        r.slug,
                        r.version,
                        r.name,
                        r.input_contract
                            .map(|c| c.to_string())
                            .unwrap_or("-".into()),
                        r.output_contract
                            .map(|c| c.to_string())
                            .unwrap_or("-".into())
                    );
                }
            }
        }
        RoleCmd::Get { slug } => match ctx.api().role_get(&slug).map_err(render_api_error)? {
            Some(r) => outln!("{}", serde_json::to_string_pretty(&r)?),
            None => bail!("role `{slug}` not found"),
        },
    }
    Ok(())
}

fn agent(ctx: &Ctx, cmd: AgentCmd, json: bool) -> Result<()> {
    match cmd {
        AgentCmd::Add {
            name,
            provider,
            model,
            runtime,
        } => {
            let a = ctx
                .api()
                .agent_add(AgentAddReq {
                    name,
                    provider,
                    model,
                    runtime,
                })
                .map_err(render_api_error)?;
            outln!(
                "agent {} registered (credentials stay outside the store)",
                a.name
            );
        }
        AgentCmd::List => {
            // (API agent.list read.)
            let as_ = ctx.api().agent_list().map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&as_)?);
            } else {
                for a in as_ {
                    outln!(
                        "{}  provider={}  model={}  runtime={}",
                        a.name,
                        a.provider,
                        a.model,
                        a.runtime
                    );
                }
            }
        }
    }
    Ok(())
}

fn contract(ctx: &Ctx, cmd: ContractCmd, json: bool) -> Result<()> {
    match cmd {
        ContractCmd::Add {
            name,
            version,
            file,
        } => {
            let schema = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            serde_json::from_str::<serde_json::Value>(&schema)
                .context("contract file must be valid JSON")?;
            let r = ctx
                .api()
                .contract_add(ContractAddReq {
                    name,
                    version,
                    schema,
                })
                .map_err(render_api_error)?;
            outln!("contract {}@{} registered", r.name, r.version);
        }
        ContractCmd::Get { name, version } => match ctx
            .api()
            .contract_get(&name, version)
            .map_err(render_api_error)?
        {
            Some(s) => {
                if json {
                    outln!("{s}");
                } else {
                    // QA R1b: no unwrap — a corrupt stored schema propagates
                    // through the render path as an operator-visible error
                    // instead of panicking the binary.
                    let v: serde_json::Value = serde_json::from_str(&s)
                        .with_context(|| format!("stored contract {name}@{version} schema is not valid JSON — re-register it with `devai contract add`"))?;
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
        ExecCmd::Start {
            id,
            role,
            agent,
            inputs,
        } => {
            // The API `exec.start` operation validates the role/stage
            // matrix, runs the labeled single-writer refusal (REQ-SW-02
            // AC2 — the enforcement point stays the store index), computes
            // the workspace revision and the owner columns platform-side
            // (REQ-001/REQ-006, REQ-SW-03) and opens the row as its own
            // root attempt (REQ-FG-06) — exactly the path the CLI drove
            // directly before cycle 2.
            let s = ctx
                .api()
                .exec_start(ExecStartReq {
                    id,
                    role,
                    agent,
                    inputs,
                })
                .map_err(render_api_error)?;
            outln!(
                "execution {} for role {}@{} started in state {}",
                short(&s.execution.id),
                s.execution.role_slug,
                s.execution.role_version,
                s.task.state.as_str()
            );
            outln!(
                "publish its output artifact with: devai artifact publish {} --execution {}",
                short(&s.task.id),
                short(&s.execution.id)
            );
        }
        ExecCmd::Finish { exec_id, summary } => {
            let e = ctx
                .api()
                .exec_finish(&exec_id, summary)
                .map_err(render_api_error)?;
            outln!("execution {} completed", short(&e.id));
        }
        ExecCmd::Fail { exec_id, error } => {
            let e = ctx
                .api()
                .exec_fail(&exec_id, &error)
                .map_err(render_api_error)?;
            outln!(
                "execution {} failed (task workflow state unchanged; the stage may be retried)",
                short(&e.id)
            );
        }
        ExecCmd::Cancel { exec_id } => {
            let e = ctx.api().exec_cancel(&exec_id).map_err(render_api_error)?;
            outln!(
                "execution {} cancelled (cooperative stop acknowledged)",
                short(&e.id)
            );
        }
        ExecCmd::List { task } => {
            // (API exec.list read.)
            let es = ctx
                .api()
                .exec_list(task.as_deref())
                .map_err(render_api_error)?;
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
                    outln!(
                        "{}  {:<22} {:<12} {:<10}{} {}",
                        short(&e.id),
                        e.role_slug,
                        short(&e.task_id),
                        e.status.as_str(),
                        flags,
                        e.summary
                            .as_deref()
                            .or(e.error.as_deref())
                            .map(|s| ellipsis(s, 60))
                            .unwrap_or_default()
                    );
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
            let v = ctx
                .api()
                .exec_liveness(&exec_id)
                .map_err(render_api_error)?;
            if json {
                outln!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                outln!(
                    "execution {}  status {}  owner pid={:?} host={:?}  liveness: {}",
                    v.id,
                    v.status,
                    v.owner_pid,
                    v.owner_host,
                    v.liveness
                );
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
        outln!(
            "task {} — {} events, {} artifacts, {} executions, {} roles, {} contracts",
            short(&manifest.task_id),
            manifest.counts.events,
            manifest.counts.artifacts,
            manifest.counts.executions,
            manifest.counts.roles,
            manifest.counts.contracts
        );
        outln!("manifest carries sha256 for every file; caches and credentials are never included");
    }
    Ok(())
}

fn import(ctx: &Ctx, dir: &Path, json: bool) -> Result<()> {
    // (API import.bundle — the store side-effect moved behind the operation;
    // single-CURRENT and single-writer invariants are enforced inside.)
    let o = ctx.api().import_bundle(dir).map_err(render_api_error)?;
    if json {
        outln!(
            "{}",
            serde_json::json!({"task_id": o.task_id.to_string(), "artifacts": o.imported_artifacts, "events": o.imported_events, "skipped_existing": o.skipped_existing})
        );
    } else if o.skipped_existing {
        outln!(
            "task {} already present — nothing imported",
            short(&o.task_id)
        );
    } else {
        outln!(
            "task {} imported: {} artifacts, {} events restored",
            short(&o.task_id),
            o.imported_artifacts,
            o.imported_events
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn print_task(t: &Task) {
    outln!("id:        {}", t.id);
    outln!("title:     {}", t.title);
    outln!(
        "state:     {}{}",
        t.state.as_str(),
        t.return_state
            .map(|r| format!(" (returns to {})", r.as_str()))
            .unwrap_or_default()
    );
    outln!("requirements: v{}", t.requirements_seq);
    outln!(
        "project:   {}",
        t.project_id
            .map(|i| short(&i))
            .unwrap_or_else(|| "-".into())
    );
    outln!("lock:      {}", t.lock_version);
    outln!(
        "created:   {}  updated: {}",
        t.created_at.format("%Y-%m-%d %H:%M:%S"),
        t.updated_at.format("%Y-%m-%d %H:%M:%S")
    );
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
        t.return_state
            .map(|r| format!(" (returns to {})", r.as_str()))
            .unwrap_or_default()
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
        outln!(
            "cycle ended in {} — terminal state, no open pipeline step",
            t.state.as_str()
        );
    } else if v.pipeline.off_spine {
        // REWORK loop / hold state: the bracket marks where it re-enters.
        match v.pipeline.position.as_deref() {
            Some(p) => outln!(
                "note:  {} is off the spine — it returns to [{}]",
                t.state.as_str(),
                p
            ),
            None => outln!(
                "note:  {} holds no remembered stage (state corruption — check `devai task events`)",
                t.state.as_str()
            ),
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
        if ops.is_empty() {
            "(none — task is terminal)".to_string()
        } else {
            ops.join(" | ")
        }
    );
    Ok(())
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

pub(crate) fn short(id: &Uuid) -> String {
    // The canonical width + rationale live in devai_core::types::ID_PREFIX_CHARS
    // (single source, shared with devai-api — R1 QA no-magic).
    devai_core::types::short_id(id)
}

/// Display-short of an id that already arrives as a string (API views).
pub(crate) fn short_str(id: &str) -> String {
    devai_core::types::short_id_str(id)
}
