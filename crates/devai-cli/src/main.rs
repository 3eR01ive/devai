//! `devai` — local-first multi-agent development platform, CLI interface.
//!
//! The CLI is a thin translation layer: it maps user/agent intent onto
//! domain commands, hands them to `devai-core` for validation and persists
//! through `devai-store`. No workflow logic lives here.

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
        /// Agent name from `devai agent list` (builtin: mock, claude)
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
