//! `devai` — local-first multi-agent development platform, CLI interface.
//!
//! The CLI is a thin translation layer: it maps user/agent intent onto
//! domain commands and executes them as typed operations of the platform
//! API (`devai-api`). Since cycle 2 it holds no store handle at all —
//! reads and mutations both go through `Api`, and this layer renders
//! results. No workflow logic lives here.

mod commands;
mod out;

use clap::{Parser, Subcommand};
use commands::Ctx;

#[derive(Parser)]
#[command(name = "devai", version, about = "Devai: автономная local-first платформа мультиагентной разработки", long_about = None)]
struct Cli {
    /// Data directory (default: $DEVAI_HOME or ~/.devai)
    #[arg(long, global = true, env = "DEVAI_HOME")]
    home: Option<std::path::PathBuf>,

    /// Actor identity recorded in events/decisions (default: $DEVAI_USER or $USER)
    #[arg(long, global = true, env = "DEVAI_USER")]
    actor: Option<String>,

    /// Machine-readable JSON output where supported
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize the local store (~/.devai) and seed the starter roles.
    /// Re-running after editing config YAML version-bumps the changed roles
    /// — new executions pick up the new version, history stays intact.
    Init {
        /// Config tree directory (default: <home>/config, seeded from built-in defaults)
        #[arg(long)]
        config: Option<std::path::PathBuf>,
    },
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Project RULES ratification (task 01a1bacf): a HUMAN act that gives a
    /// project's `profiles/<id>.overlay.yaml` edition force. The platform
    /// computes the edition fingerprint itself; no engine, role or agent
    /// path can create these decisions.
    #[command(subcommand)]
    Profile(ProfileCmd),
    #[command(subcommand)]
    Task(TaskCmd),
    #[command(subcommand)]
    Artifact(ArtifactCmd),
    #[command(subcommand)]
    Question(QuestionCmd),
    #[command(subcommand)]
    Role(RoleCmd),
    #[command(subcommand)]
    Agent(AgentCmd),
    /// The model-bundle surface (task 01a1b71f): what is in force, the
    /// declared bundles, a real probe, and the ONE-step switch.
    #[command(subcommand)]
    Model(ModelCmd),
    #[command(subcommand)]
    Contract(ContractCmd),
    #[command(subcommand)]
    Exec(ExecCmd),
    /// The published API operation registry — the contract every consumer
    /// (CLI first) is built on. Read-only, machine-complete.
    #[command(subcommand)]
    Api(ApiCmd),
    /// PLATFORM INTERNAL: one detached role runner (REQ-AR-01/03). Started by
    /// `task run` in its own session; it opens the execution row, writes the
    /// handshake the caller waits for, drives the chain and records the exit
    /// code and output tail. Not an operator command.
    #[command(hide = true, name = "__supervise")]
    Supervise {
        #[arg(long)]
        task: String,
        #[arg(long)]
        run: String,
        #[arg(long)]
        ack: std::path::PathBuf,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        #[arg(long)]
        force_reaudit: bool,
    },
    /// Export a task bundle for portability (domain data + artifacts, no caches)
    Export {
        #[arg(long)]
        task: String,
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// Import a task bundle produced by export
    Import {
        #[arg(long)]
        dir: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Register a project (optionally with local repository paths)
    Add {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Paths of local git repositories
        #[arg(long)]
        repo: Vec<String>,
    },
    List,
    Get {
        /// Project id or name
        id: String,
    },
}

/// Project rules ratification — the HUMAN door to the project overlay
/// (task 01a1bacf). Every verb names the project explicitly; the edition
/// fingerprint is computed by the platform, never typed by the operator.
#[derive(Subcommand)]
enum ProfileCmd {
    /// Current rules state: the edition fingerprint and every item's force
    /// (in force with the decision/actor, or OUT with the reason). Read-only.
    Status {
        /// Project id or name
        project: String,
    },
    /// HUMAN-ONLY: ratify the CURRENT edition of a project's rules file,
    /// covering the named items. Records the `profile_ratification` decision
    /// (project, edition fingerprint, items, actor, rationale) and writes the
    /// pointers into the file.
    Ratify {
        /// Carrier task of the decision (id or unique prefix)
        #[arg(long)]
        task: String,
        /// Project id or name the rules belong to
        #[arg(long)]
        project: String,
        /// Covered item ids (comma-separated); repeatable
        #[arg(long, value_delimiter = ',')]
        items: Vec<String>,
        /// Cover EVERY item of the current edition (the decision still stores
        /// the full explicit list)
        #[arg(long)]
        all: bool,
        #[arg(long)]
        rationale: Option<String>,
    },
    /// HUMAN-ONLY: revoke an APPROVED ratification (items leave force; the
    /// rules file is not touched).
    Revoke {
        /// Ratification decision (id or unique prefix)
        decision: String,
        #[arg(long)]
        rationale: Option<String>,
    },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Create a task (DRAFT). A task is owned by the platform from birth.
    New {
        title: String,
        #[arg(long)]
        project: Option<String>,
        /// Business description of the need
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long)]
        description_file: Option<std::path::PathBuf>,
        /// Task ids this task was DERIVED from ('parent' edges), registered
        /// at the moment of creation — the "exists because" never leaves
        /// for the operator chat (REQ-DEP-07). Comma-separated prefixes ok.
        #[arg(long, value_delimiter = ',')]
        parent: Vec<String>,
        /// Task ids blocking this one ('blocks' edges: resume/acceptance
        /// stay refused until they close). Comma-separated prefixes ok.
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<String>,
        /// Retry guard (REQ-API-02, ratified OQ-1): a repeated `task new`
        /// carrying the SAME key returns the task the first committed
        /// create registered instead of duplicating it — the recovery path
        /// when a successful create's response was lost.
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    List,
    Get {
        id: String,
    },
    /// Current state + the operations the platform permits next
    Status {
        id: String,
    },
    /// The signable surface of an APPROVAL card: the spec paragraph,
    /// requirements, document open questions, reviewer block, previous-version
    /// diff, the two signature rights and the newest stored signature fact —
    /// the same ONE object the window's signature card renders
    /// (with --json: the machine view).
    Signature {
        id: String,
    },
    /// Pipeline position of the task, allowed operations and a snapshot of
    /// running executions — state at a glance without copy-pasting ids
    /// (with --json: the same snapshot for machines).
    Progress {
        id: String,
    },
    /// What is running right now, across all tasks of this store: short task
    /// id, role@version, elapsed, workflow stage, worktree link — a read-only
    /// snapshot, no waiting (with --json: one machine snapshot). Per-task
    /// pipeline detail stays with `task progress`.
    Active,
    /// What the task's isolated workspace CHANGES against the bound baseline:
    /// committed file list with A/M/D markers, the stored range (baseline..tip)
    /// and per-run commit stamps. Read-only view over the task's branch —
    /// states no_isolation / live / historical / unavailable are verdicts in
    /// data, never errors (with --json: the full machine view).
    Workspace {
        id: String,
    },
    /// Readable unified diff of ONE changeset-listed file of the task's
    /// rework branch against the stored baseline — the same range the
    /// `task workspace` list used (with --json: the machine view).
    WorkspaceDiff {
        id: String,
        path: String,
        /// Print the whole file as context instead of three lines around
        /// each hunk — the window unfolds hidden lines from this view.
        #[arg(long)]
        full: bool,
    },
    /// Show the CURRENT quality report — the rendering a human signs at the
    /// QA gate (with --json: the full machine report).
    Report {
        id: String,
    },
    /// DRAFT → ANALYSIS: start the workflow
    Start {
        id: String,
    },
    /// Run the assigned roles through all autonomous stages until the next
    /// human gate (APPROVAL / final acceptance), hold, or terminal state.
    Run {
        id: String,
        /// Agent name from `devai agent list` (builtin: mock, claude;
        /// other providers — e.g. hermes — via `devai agent add --provider hermes`).
        /// OMITTED: the agent of the ACTIVE model bundle runs; with no marked
        /// bundle the run refuses typed instead of falling back to `mock`.
        #[arg(long)]
        agent: Option<String>,
        /// Workspace where QA gates and mechanical probes execute (default: cwd).
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        /// Explicit human decision to re-audit despite a FRESH quality
        /// report (the report is bound to exactly the current workspace
        /// revision). Persists an override decision + event; never implied.
        #[arg(long)]
        force_reaudit: bool,
        /// WAIT for the launched run to finish instead of returning at once
        /// (REQ-AR-04: the default is the handle; waiting is separate).
        #[arg(long)]
        wait: bool,
    },
    /// Publish/replace a REQUIREMENTS_SPEC and move to REVIEW
    Revise {
        id: String,
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Record the reviewer's verdict in REVIEW
    Review {
        id: String,
        #[arg(long)]
        verdict: String,
        #[arg(long)]
        summary: Option<String>,
    },
    /// APPROVAL → IMPLEMENTATION (requires approval rights: human/approver only)
    Approve {
        id: String,
        #[arg(long)]
        rationale: Option<String>,
    },
    /// APPROVAL → ANALYSIS
    Reject {
        id: String,
        #[arg(long)]
        rationale: Option<String>,
    },
    /// QA → ACCEPTED (requires a CURRENT QUALITY_REPORT)
    Accept {
        id: String,
        #[arg(long)]
        rationale: Option<String>,
    },
    /// QA → REWORK (return for fixes)
    Return {
        id: String,
        #[arg(long)]
        rationale: Option<String>,
    },
    /// The EMPTY-REWORK GATE escape (human-only, REQ-RG-03): the task sits on
    /// a `qa.rework_gate_tripped` hold because a REWORK publication named the
    /// SAME platform-stamped commit as the previous report — the rework
    /// brought no new work. This command records an explicit
    /// `rework_noop_ratified` decision for exactly that report pair, releases
    /// the gate question, and runs the withheld REWORK → QA transition citing the
    /// decision. Only a human invocation can ratify; no agent or engine path
    /// can create the decision, and the ratification binds to the one cycle's
    /// pair — a later identical-commit rework trips the gate again.
    RatifyEmptyRework {
        id: String,
        #[arg(long)]
        rationale: Option<String>,
    },
    /// The MERGE GATE (human-only): merge the task's platform-created
    /// `rework/<task-id>` branch into the workspace's current baseline
    /// branch. The platform never merges by itself — this command is the
    /// single merge path, records a `rework_merge` decision + event, and
    /// moves no task state. Conflicts stay with the human who invoked it.
    Merge {
        id: String,
        /// Repository whose checked-out branch is the merge baseline
        /// (default: current directory).
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        #[arg(long)]
        rationale: Option<String>,
    },
    Pause {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Resume a PAUSED/BLOCKED/FAILED task, or answer-driven restore
    Resume {
        id: String,
    },
    Block {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Cancel {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Пройти путь заново с названной стадии (только человек): результаты
    /// этой стадии и всех следующих помечаются STALE, причина ложится в журнал
    Rewind {
        id: String,
        /// Куда вернуться: STATEMENT, ANALYSIS, REVIEW, APPROVAL,
        /// IMPLEMENTATION или QA
        #[arg(long = "to")]
        to: String,
        #[arg(long)]
        reason: String,
    },
    /// Переписать изначальную постановку задачи (только человек). На начатом
    /// пути это перепройти с STATEMENT: результаты ниже помечаются STALE
    Restate {
        id: String,
        #[arg(long)]
        title: Option<String>,
        /// Постановка целиком; `-` читать из stdin
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        reason: String,
    },
    /// Record a runtime failure (task parks in FAILED; `resume` retries)
    Fail {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Assign or REPLACE the owning project of an existing task
    /// (REQ-BIND-02, C5 D1): refs resolve first, an unknown task or
    /// project refuses, a re-apply of the already-bound project is a
    /// documented no-op. One append-only `task.project.set` event per
    /// effective change; workflow state is not touched.
    SetProject {
        /// The task (id or unique prefix)
        id: String,
        /// The project (id or name)
        project: String,
    },
    /// Backfill the project binding (REQ-BIND-03 under the ratified
    /// OQ-BIND-2 rule): bind the given project to every task that lives
    /// UNBOUND and NON-TERMINAL in this store. Terminal history keeps the
    /// address it lived and judged at; tasks of another project are never
    /// re-bound. Every binding goes through the `task.project.set`
    /// operation; running it twice changes nothing the second time.
    BackfillProject {
        /// The project (id or name)
        project: String,
    },
    /// Set or REPLACE the human name (signboard) of a task directly
    /// (task 01a1ad1d, ratified OQ-TN-1): the human door to the name —
    /// no rework cycle, the workflow state is not touched. The name is
    /// validated against the same declarative window as spec publication
    /// (`config/workflow/task-name.yaml`: non-blank, min..max chars, one
    /// line); the formal title of the creator is never rewritten — it
    /// stays in the task data. One append-only `task.human_name.set`
    /// event per effective change; re-applying the current name is a
    /// side-effect-free no-op.
    Rename {
        /// The task (id or unique prefix)
        id: String,
        /// The new human name (short, one line)
        name: String,
        /// Reject if the task's lock version moved (optimistic concurrency)
        #[arg(long)]
        expected_lock: Option<u64>,
    },
    /// Show immutable event history
    Events {
        id: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Decision rows recorded for the task (requirements approvals,
    /// dependency overrides, gate ratifications, merges) — the human acts
    /// as data, through the API task.decisions operation
    Decisions {
        id: String,
    },
    /// Register a typed task dependency: 'parent' ("породила" — why this
    /// task exists, pure genealogy, never gates — approved OQ3) or 'blocks'
    /// ("не может продолжиться без той" — resume and acceptance refused
    /// while open; closed only when the blocker is ACCEPTED, approved OQ1).
    /// Works at creation (--parent / --blocked-by) and retroactively, from
    /// either moment (REQ-DEP-07). Cycles are rejected WITHIN each kind
    /// (approved OQ4); a mixed parent↔blocks cycle is legal.
    Depend {
        /// The dependent task (id or unique prefix)
        id: String,
        /// The task it depends on (id or unique prefix)
        #[arg(long)]
        on: String,
        /// Dependency kind: parent | blocks
        #[arg(long)]
        kind: String,
    },
    /// Typed dependencies of one task: what it depends on (outgoing) and
    /// what depends on it (incoming), ACTIVE edges only. An epic is not a
    /// separate entity — per the ratified OQ8 the epic VERTEX (no outgoing
    /// 'parent', incoming ones) reads out of this same dependency view
    /// (REQ-DEP-06); `task epics` lists the whole registry.
    Dependencies {
        id: String,
    },
    /// Supersede (the approved removal, OQ7: edges are immutable) an
    /// ACTIVE dependency edge. The row stays in the store as SUPERSEDED
    /// history with full attribution; the live graph stops seeing it, so a
    /// gate it held opens and the same relation may be re-registered fresh.
    Unlink {
        /// The dependent task (id or unique prefix)
        id: String,
        /// The task it depends on (id or unique prefix)
        #[arg(long)]
        on: String,
        /// Dependency kind to supersede: parent | blocks
        #[arg(long)]
        kind: String,
    },
    /// Human-escape of the dependency gate (approved OQ2): record an
    /// explicit ratification to continue DESPITE open 'blocks'
    /// dependencies. Stored as a `dependency_override` decision (verdict
    /// APPROVED, refs = the covered blocker ids). The refusal itself stays
    /// unconditional for anything not covered — a NEW blocker re-opens the
    /// gate; nothing ever resumes or accepts automatically by itself.
    Override {
        /// The task whose gate is ratified (id or unique prefix)
        id: String,
        /// Blocker task ids this ratification covers; omit to cover
        /// exactly the blockers open right now.
        #[arg(long = "blocker", value_delimiter = ',')]
        blocker: Vec<String>,
        /// The operator's stated reason for continuing despite
        #[arg(long)]
        rationale: Option<String>,
    },
    /// Epic registry as a VIEW over the dependency graph (approved OQ8):
    /// epic = task vertex with no outgoing ACTIVE 'parent' edge and at
    /// least one incoming one. No separate epic entity exists — this is the
    /// same stored dependency data, one query (REQ-DEP-06).
    Epics,
    /// The task dependency graph with task states — planning-level "what
    /// is happening": every node, its outgoing typed edges and whether an
    /// open 'blocks' edge still holds it. Built purely from stored data
    /// (REQ-DEP-04/05); the roadmap orders itself out of this graph.
    Graph,
}

#[derive(Subcommand)]
enum ArtifactCmd {
    /// Publish a versioned artifact. Certain kinds advance the workflow
    /// (REQUIREMENTS_SPEC → REVIEW, IMPLEMENTATION_REPORT → QA, …).
    Publish {
        id: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Artifact ids this result depends on
        #[arg(long, value_delimiter = ',')]
        depends: Vec<String>,
        #[arg(long)]
        contract: Option<String>,
        /// Mark as produced by a role execution
        #[arg(long)]
        execution: Option<String>,
        /// Verdict for REVIEW_REPORT publishing
        #[arg(long)]
        verdict: Option<String>,
        #[arg(long, default_value = "artifact")]
        category: String,
    },
    List {
        id: String,
    },
    /// Print artifact content to stdout
    Cat {
        artifact_id: String,
    },
}

#[derive(Subcommand)]
enum QuestionCmd {
    /// A role asks the user a question; blocking questions park the task
    Ask {
        id: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        context: Option<String>,
        #[arg(long, value_delimiter = ',')]
        options: Vec<String>,
        #[arg(long)]
        blocking: bool,
        #[arg(long)]
        execution: Option<String>,
    },
    Answer {
        question_id: String,
        #[arg(long)]
        text: String,
    },
    List {
        id: String,
    },
}

#[derive(Subcommand)]
enum RoleCmd {
    /// Register a versioned RoleDefinition from a JSON file
    Add {
        #[arg(long)]
        file: std::path::PathBuf,
    },
    List,
    Get {
        slug: String,
    },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Register a local AgentInstance (provider/model/runtime; keys stay out of the store)
    Add {
        name: String,
        #[arg(long, default_value = "")]
        provider: String,
        #[arg(long, default_value = "")]
        model: String,
        #[arg(long, default_value = "local")]
        runtime: String,
    },
    List,
}

/// The model-bundle CLI mirror of the UI section (REQ-MS-07): the SAME three
/// operations, the SAME verdict vocabulary — the console never invents a
/// second reading of the same data.
#[derive(Subcommand)]
enum ModelCmd {
    /// The section in one read: the bundle in force, ONE status word with the
    /// probe's latency and time, every declared bundle, the history, the
    /// refusal banner and the runs in flight.
    List,
    /// Probe ONE declared bundle with a real call and show the typed verdict.
    Probe {
        /// Bundle id from `devai model list`
        id: String,
    },
    /// Switch the platform to ONE declared bundle: probe first, apply only on
    /// `ok`, atomically, with history. Refusals are typed.
    Switch {
        /// Bundle id from `devai model list`
        id: String,
        /// Why the switch is made (manual|limits|error); the platform never
        /// derives it.
        #[arg(long, default_value = "manual")]
        reason: String,
    },
}

#[derive(Subcommand)]
enum ContractCmd {
    /// Register a versioned contract schema (JSON file)
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 1)]
        version: u32,
        #[arg(long)]
        file: std::path::PathBuf,
    },
    Get {
        name: String,
        #[arg(long, default_value_t = 1)]
        version: u32,
    },
}

#[derive(Subcommand)]
enum ExecCmd {
    /// Start a role execution. The platform validates the role/stage matrix
    /// before anything runs.
    Start {
        id: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_delimiter = ',')]
        inputs: Vec<String>,
    },
    Finish {
        exec_id: String,
        #[arg(long)]
        summary: Option<String>,
    },
    /// Консоль прогона: что роль пишет в свои потоки (хвост с потолком).
    /// Тот же поток, что показывает окно: улики прогона, без пересказа.
    Transcript {
        id: String,
        #[arg(long)]
        execution: String,
    },
    Fail {
        exec_id: String,
        #[arg(long)]
        error: String,
    },
    Cancel {
        exec_id: String,
    },
    List {
        #[arg(long)]
        task: Option<String>,
        /// Only the executions of one RUN, including the later ones a chain
        /// produced (REQ-AR-04 AC2).
        #[arg(long)]
        run: Option<String>,
    },
    /// Close executions whose owner process is PROVABLY gone (REQ-AR-05):
    /// host restarted, pid vanished, or a pre-ownership RUNNING row. A typed
    /// report of what was closed and why; nothing is killed.
    Reap {
        #[arg(long)]
        task: Option<String>,
    },
    /// One execution row by id/prefix — observable without the launching
    /// terminal (REQ-API-07, via the API exec.get operation)
    Get {
        exec_id: String,
    },
    /// Liveness of the recorded owner of an execution: alive | dead |
    /// unknown — orphans distinguishable by stored data, not pgrep
    /// (REQ-API-07 AC3, via the API exec.liveness operation; read-only,
    /// never signals the process)
    Liveness {
        exec_id: String,
    },
}

#[derive(Subcommand)]
enum ApiCmd {
    /// Print the published operation registry: every API operation with its
    /// typed input/output shapes and textual semantics (REQ-API-06).
    Registry,
}

fn main() {
    // REQ-PIPE-01 backstop: all CLI output goes through `out`, which already
    // dies quietly on a closed reader. This hook covers stdout writes outside
    // that channel (clap's built-in help/version printing): a panic caused by
    // EPIPE while printing to stdout exits silently instead of showing panic
    // text. Any other panic keeps the standard panic report on stderr.
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg: &str = match payload.downcast_ref::<String>() {
            Some(s) => s,
            None => payload.downcast_ref::<&str>().copied().unwrap_or_default(),
        };
        if msg.contains("failed printing to stdout") && msg.contains("Broken pipe") {
            std::process::exit(0);
        }
        eprintln!("devai: panic: {info}");
        std::process::exit(101);
    }));

    let cli = Cli::parse();
    let ctx = match Ctx::new(cli.home.as_deref(), cli.actor.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("devai: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = commands::run(&ctx, cli.command, cli.json) {
        eprintln!("devai: {e}");
        std::process::exit(1);
    }
}
