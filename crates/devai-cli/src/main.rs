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
    #[command(subcommand)]
    Contract(ContractCmd),
    #[command(subcommand)]
    Exec(ExecCmd),
    /// The published API operation registry — the contract every consumer
    /// (CLI first) is built on. Read-only, machine-complete.
    #[command(subcommand)]
    Api(ApiCmd),
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
    },
    List,
    Get {
        id: String,
    },
    /// Current state + the operations the platform permits next
    Status {
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
        /// other providers — e.g. hermes — via `devai agent add --provider hermes`)
        #[arg(long, default_value = "mock")]
        agent: String,
        /// Workspace where QA gates and mechanical probes execute (default: cwd).
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        /// Explicit human decision to re-audit despite a FRESH quality
        /// report (the report is bound to exactly the current workspace
        /// revision). Persists an override decision + event; never implied.
        #[arg(long)]
        force_reaudit: bool,
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
    /// Record a runtime failure (task parks in FAILED; `resume` retries)
    Fail {
        id: String,
        #[arg(long)]
        reason: Option<String>,
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
