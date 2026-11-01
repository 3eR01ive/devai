use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_runtime::{Engine, Gate};
use devai_store::{ArtifactDraft, Store, guess_media_type};
use uuid::Uuid;

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
        println!("{}", serde_json::json!({"home": ctx.home, "config": cfg_path, "roles_seeded": seeded}));
    } else {
        println!("devai home: {}", ctx.home.display());
        println!("config tree: {}", cfg_path.display());
        if seeded > 0 {
            println!("seeded/updated {seeded} role version(s) from config");
            println!("edit {}/criteria/*.yaml or roles/*.yaml and re-run `devai init` to bump versions", cfg_path.display());
        } else {
            println!("store already initialized (config unchanged)");
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
            println!("project {} ({})", p.name, short(&p.id));
        }
        ProjectCmd::List => {
            let ps = ctx.store.list_projects()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&ps)?);
            } else if ps.is_empty() {
                println!("(no projects)");
            } else {
                for p in ps {
                    println!("{}  {}  repos={}", short(&p.id), p.name, p.repos.len());
                }
            }
        }
        ProjectCmd::Get { id } => match ctx.store.get_project(&id)? {
            Some(p) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&p)?);
                } else {
                    println!("id:          {}", p.id);
                    println!("name:        {}", p.name);
                    println!("description: {}", p.description);
                    for r in p.repos {
                        println!("repo:        {r}");
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
                println!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                println!("task {} created in DRAFT — workflow is owned by devai", short(&t.id));
                println!("start it with: devai task start {}", short(&t.id));
            }
        }
        TaskCmd::List => {
            let ts = ctx.store.list_tasks()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&ts)?);
            } else if ts.is_empty() {
                println!("(no tasks)");
            } else {
                for t in ts {
                    println!("{}  {:<17}  v{:>2}  {}", short(&t.id), t.state.as_str(), t.requirements_seq, t.title);
                }
            }
        }
        TaskCmd::Get { id } => {
            let t = ctx.store.resolve_task(&id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&t)?);
            } else {
                print_task(&t);
            }
        }
        TaskCmd::Status { id } => status(ctx, &id, json)?,
        TaskCmd::Start { id } => op(ctx, &id, |t, _a| workflow::start(t, &ctx.actor, ctx.now()))?,
        TaskCmd::Run { id, agent } => task_run(ctx, &id, &agent, json)?,
        TaskCmd::Revise { id, file, reason } => revise(ctx, &id, &file, reason.as_deref())?,
        TaskCmd::Review { id, verdict, summary } => {
            let v = parse_verdict(&verdict)?;
            op(ctx, &id, move |t, _a| workflow::review_recorded(t, v, summary.as_deref(), &ctx.actor, ctx.now()))?;
        }
        TaskCmd::Approve { id, rationale } => {
            op(ctx, &id, move |t, a| {
                if a.iter().any(|x| x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current) {
                    workflow::approve_requirements(t, rationale.as_deref(), &ctx.actor, ctx.now())
                } else {
                    Err(devai_core::CoreError::guard("no CURRENT REQUIREMENTS_SPEC artifact to approve"))
                }
            })?;
        }
        TaskCmd::Reject { id, rationale } => op(ctx, &id, move |t, _a| workflow::reject_requirements(t, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Accept { id, rationale } => op(ctx, &id, move |t, a| workflow::accept(t, a, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Return { id, rationale } => op(ctx, &id, move |t, _a| workflow::return_to_rework(t, rationale.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Pause { id, reason } => op(ctx, &id, move |t, _a| workflow::pause(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Resume { id } => op(ctx, &id, |t, _a| workflow::resume(t, &ctx.actor, ctx.now()))?,
        TaskCmd::Block { id, reason } => op(ctx, &id, move |t, _a| workflow::block(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Cancel { id, reason } => op(ctx, &id, move |t, _a| workflow::cancel(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Fail { id, reason } => op(ctx, &id, move |t, _a| workflow::fail(t, reason.as_deref(), &ctx.actor, ctx.now()))?,
        TaskCmd::Events { id, limit } => {
            let t = ctx.store.resolve_task(&id)?;
            let evs = ctx.store.events(t.id, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&evs)?);
            } else {
                for e in evs {
                    println!("{}  {:<28} {:<12} {}", e.created_at.format("%Y-%m-%d %H:%M:%S"), e.kind, e.actor, compact(&e.payload));
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
        println!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
    } else {
        println!("recorded ({})", m.events[0].kind);
    }
    if staled > 0 {
        println!("marked STALE: {staled} dependent artifact(s) — history preserved");
    }
    if superseded > 0 {
        println!("superseded: {superseded}");
    }
    let ops = workflow::allowed_ops(&m.task);
    if !ops.is_empty() {
        println!("next: {}", ops.join(" | "));
    }
    Ok(())
}

/// Drive the task through every AUTONOMOUS stage with a real role
/// execution. Stops at human gates (APPROVAL, acceptance) — never past them.
fn task_run(ctx: &Ctx, id: &str, agent: &str, json: bool) -> Result<()> {
    let engine = Engine::new(&ctx.store);
    let report = engine.advance(id, agent, &ctx.actor).map_err(anyhow::Error::new)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for s in &report.steps {
        println!(
            "{} → {}: `{}` on `{}`  artifact {} ({})",
            s.from.as_str(),
            s.to.as_str(),
            s.role,
            s.agent,
            short(&s.artifact_id),
            s.artifact_kind.as_str(),
        );
        if let Some(sum) = &s.summary {
            println!("   ↳ {}", ellipsis(sum, 160));
        }
    }
    match report.gate {
        Gate::WaitingApproval => println!("⏸ APPROVAL is a human gate — approve or reject the requirements"),
        Gate::WaitingAccept => println!("⏸ acceptance is a human gate — accept or return the result"),
        Gate::Terminal(st) => println!("task finished: {}", st.as_str()),
        Gate::Hold(st) => println!("task held in {} — release it first", st.as_str()),
    }
    let t = ctx.store.get_task(&report.task_id.to_string())?;
    let ops = workflow::allowed_ops(&t);
    if !ops.is_empty() {
        println!("next: {}", ops.join(" | "));
    }
    Ok(())
}

fn revise(ctx: &Ctx, id: &str, file: &Path, reason: Option<&str>) -> Result<()> {
    let task = ctx.store.resolve_task(id)?;
    let (kind, name) = (ArtifactKind::RequirementsSpec, file.display().to_string());
    let content = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    let _a = publish_raw(ctx, task.id, kind, &name, content, None, None, vec![], None, DataCategory::Artifact)?;
    let after = ctx.store.artifacts(task.id)?;
    let m = workflow::requirements_published(&task, &after, reason, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
    ctx.store.commit(task.lock_version, &m)?;
    println!("requirements v{} published → REVIEW", m.task.requirements_seq);
    let staled: Vec<_> = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Stale).map(|(i, _)| i.to_string()).collect();
    if !staled.is_empty() {
        println!("consequences: {} artifact(s) marked STALE:", staled.len());
        for s in &staled {
            println!("  stale  {s}");
        }
        println!("old versions are kept in history; nothing was deleted");
    }
    println!("next: {}", workflow::allowed_ops(&m.task).join(" | "));
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
        println!(
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
        println!("---");
        println!("artifacts (current): {}", current.len());
        for a in &current {
            println!("  {:<22} v{:<3} {}  {}", a.kind.as_str(), a.seq, short(&a.id), a.name);
        }
        for q in &open_q {
            println!("open question {}: {}{}", short(&q.id), q.text, if q.blocking { " [BLOCKING]" } else { "" });
        }
        println!(
            "allowed operations: {}",
            if ops.is_empty() { "(none — task is terminal)".to_string() } else { ops.join(" | ") }
        );
    }
    Ok(())
}

fn print_task(t: &Task) {
    println!("id:        {}", t.id);
    println!("title:     {}", t.title);
    println!("state:     {}{}", t.state.as_str(), t.return_state.map(|r| format!(" (returns to {})", r.as_str())).unwrap_or_default());
    println!("requirements: v{}", t.requirements_seq);
    println!("project:   {}", t.project_id.map(|i| short(&i)).unwrap_or_else(|| "-".into()));
    println!("lock:      {}", t.lock_version);
    println!("created:   {}  updated: {}", t.created_at.format("%Y-%m-%d %H:%M:%S"), t.updated_at.format("%Y-%m-%d %H:%M:%S"));
    if !t.business_description.is_empty() {
        println!("business:  {}", ellipsis(&t.business_description, 200));
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
                println!("{}", serde_json::to_string_pretty(&a)?);
            } else {
                println!("artifact {} {} v{} ({})", a.kind.as_str(), short(&a.id), a.seq, a.name);
            }
            // Some kinds carry the workflow forward.
            match kind {
                ArtifactKind::RequirementsSpec => {
                    let arts = ctx.store.artifacts(task.id)?;
                    let m = workflow::requirements_published(&task, &arts, None, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
                    ctx.store.commit(task.lock_version, &m)?;
                    println!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
                }
                ArtifactKind::ReviewReport => {
                    let v = match verdict.as_deref() {
                        Some(s) => parse_verdict(s)?,
                        None => bail!("publishing REVIEW_REPORT requires --verdict approved|needs_revision"),
                    };
                    let m = workflow::review_recorded(&task, v, None, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
                    ctx.store.commit(task.lock_version, &m)?;
                    println!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
                }
                ArtifactKind::ImplementationReport => {
                    let m = workflow::implementation_reported(&task, None, &ctx.actor, ctx.now()).map_err(anyhow::Error::new)?;
                    ctx.store.commit(task.lock_version, &m)?;
                    println!("state: {} → {}", task.state.as_str(), m.task.state.as_str());
                }
                ArtifactKind::QualityReport => {
                    println!("quality report recorded; user decides via `devai task accept|return`");
                }
                _ => {}
            }
        }
        ArtifactCmd::List { id } => {
            let t = ctx.store.resolve_task(&id)?;
            let as_ = ctx.store.artifacts(t.id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&as_)?);
            } else if as_.is_empty() {
                println!("(no artifacts)");
            } else {
                for a in as_ {
                    println!("{}  {:<22} v{:<3} {:<12} {}  {}", short(&a.id), a.kind.as_str(), a.seq, a.status.as_str(), a.created_by, a.name);
                }
            }
        }
        ArtifactCmd::Cat { artifact_id } => {
            let a = ctx.store.get_artifact(&artifact_id)?;
            let bytes = ctx.store.read_blob(&a.sha256)?;
            std::io::Write::write_all(&mut std::io::stdout(), &bytes)?;
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
                println!("question {} raised; task parked in WAITING_FOR_INPUT", short(&q.id));
                println!("answer with: devai question answer {}", short(&q.id));
            } else {
                println!("question {} recorded", short(&q.id));
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
            println!("answer recorded; state: {} → {}", task.state.as_str(), m.task.state.as_str());
        }
        QuestionCmd::List { id } => {
            let t = ctx.store.resolve_task(&id)?;
            let qs = ctx.store.list_questions(t.id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&qs)?);
            } else if qs.is_empty() {
                println!("(no questions)");
            } else {
                for q in qs {
                    println!("{} {:<8} {}{}", short(&q.id), q.status.as_str(), q.text, if q.blocking && q.status == QuestionStatus::Open { " [BLOCKING]" } else { "" });
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
            println!("role {}@{} registered", r.slug, r.version);
        }
        RoleCmd::List => {
            let rs = ctx.store.list_roles()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rs)?);
            } else if rs.is_empty() {
                println!("(no roles — run `devai init`)");
            } else {
                for r in rs {
                    println!("{:<24} v{:<3} {}  in={} out={}", r.slug, r.version, r.name,
                        r.input_contract.map(|c| c.to_string()).unwrap_or("-".into()),
                        r.output_contract.map(|c| c.to_string()).unwrap_or("-".into()));
                }
            }
        }
        RoleCmd::Get { slug } => match ctx.store.get_role(&slug)? {
            Some(r) => println!("{}", serde_json::to_string_pretty(&r)?),
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
            println!("agent {name} registered (credentials stay outside the store)");
        }
        AgentCmd::List => {
            let as_ = ctx.store.list_agents()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&as_)?);
            } else {
                for a in as_ {
                    println!("{}  provider={}  model={}  runtime={}", a.name, a.provider, a.model, a.runtime);
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
            println!("contract {name}@{version} registered");
        }
        ContractCmd::Get { name, version } => match ctx.store.get_contract(&name, version)? {
            Some(s) => {
                if json {
                    println!("{s}");
                } else {
                    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
                    println!("{}", serde_json::to_string_pretty(&v)?);
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
            };
            ctx.store.start_execution(&e)?;
            println!("execution {} for role {role}@{version} started in state {}", short(&e.id), task.state.as_str());
            println!("publish its output artifact with: devai artifact publish {} --execution {}", short(&task.id), short(&e.id));
        }
        ExecCmd::Finish { exec_id, summary } => {
            let e = ctx.store.get_execution(&exec_id)?;
            if e.status != ExecutionStatus::Running {
                bail!("execution {} is {}, not RUNNING", e.id, e.status.as_str());
            }
            ctx.store.finish_execution(&e, ExecutionStatus::Completed, summary.as_deref(), None, ctx.now())?;
            println!("execution {} completed", short(&e.id));
        }
        ExecCmd::Fail { exec_id, error } => {
            let e = ctx.store.get_execution(&exec_id)?;
            if e.status != ExecutionStatus::Running {
                bail!("execution {} is {}, not RUNNING", e.id, e.status.as_str());
            }
            ctx.store.finish_execution(&e, ExecutionStatus::Failed, None, Some(&error), ctx.now())?;
            println!("execution {} failed (task workflow state unchanged; the stage may be retried)", short(&e.id));
        }
        ExecCmd::Cancel { exec_id } => {
            let e = ctx.store.get_execution(&exec_id)?;
            if e.status != ExecutionStatus::Running {
                bail!("execution {} is {}, not RUNNING", e.id, e.status.as_str());
            }
            ctx.store.finish_execution(&e, ExecutionStatus::Cancelled, None, None, ctx.now())?;
            println!("execution {} cancelled (cooperative stop acknowledged)", short(&e.id));
        }
        ExecCmd::List { task } => {
            let tid = match task.as_deref() {
                Some(id) => Some(ctx.store.resolve_task(id)?.id),
                None => None,
            };
            let es = ctx.store.list_executions(tid)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&es)?);
            } else if es.is_empty() {
                println!("(no executions)");
            } else {
                for e in es {
                    println!("{}  {:<22} {:<12} {:<10} {}", short(&e.id), e.role_slug, short(&e.task_id), e.status.as_str(),
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
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!("bundle exported to {}", out.display());
        println!("task {} — {} events, {} artifacts, {} executions, {} roles, {} contracts",
            short(&manifest.task_id), manifest.counts.events, manifest.counts.artifacts,
            manifest.counts.executions, manifest.counts.roles, manifest.counts.contracts);
        println!("manifest carries sha256 for every file; caches and credentials are never included");
    }
    Ok(())
}

fn import(ctx: &Ctx, dir: &Path, json: bool) -> Result<()> {
    let o = devai_store::bundle::import(&ctx.store, dir)?;
    if json {
        println!("{}", serde_json::json!({"task_id": o.task_id.to_string(), "artifacts": o.imported_artifacts, "events": o.imported_events, "skipped_existing": o.skipped_existing}));
    } else if o.skipped_existing {
        println!("task {} already present — nothing imported", short(&o.task_id));
    } else {
        println!("task {} imported: {} artifacts, {} events restored", short(&o.task_id), o.imported_artifacts, o.imported_events);
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

/// The CURRENT requirements spec with the highest seq, if any.
fn current_spec_id(arts: &[Artifact]) -> Option<Id> {
    arts.iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current)
        .max_by_key(|a| a.seq)
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
