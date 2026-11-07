//! The machine-readable operation registry (REQ-API-06 AC1/AC4, REQ-MUT-05).
//!
//! One entry per API operation: its name, whether it reads or mutates, the
//! typed request/response shapes by name, and a TEXTUAL description of its
//! semantics. The registry is the published contract artifact — a second
//! consumer writes against it without reading CLI code, and the description
//! is stored data, never re-derived from source at read time
//! (no-read-time-rederivation doctrine).
//!
//! Cycle 2 moved the mutation plane onto the surface: every entry whose
//! class is Mutation is working code (`crates/devai-api/src/mutations.rs`),
//! and the shape names below are the actual request/result types.

use serde::{Deserialize, Serialize};

/// Whether the operation writes to the store through workflow rules.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OpClass {
    Read,
    Mutation,
}

/// One API operation as published contract.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OpDescriptor {
    /// Stable operation name, `area.action` (e.g. `task.list`).
    pub name: &'static str,
    pub class: OpClass,
    /// Typed input shape name (`()` for no-argument reads).
    pub input: &'static str,
    /// Typed output shape name.
    pub output: &'static str,
    /// Textual semantics — what the operation does and mutates. Required
    /// for every operation (REQ-API-01 AC2: a code reference is never a
    /// substitute for a description).
    pub description: &'static str,
}

/// R3 QA (da/api-contract): THE version of the published API surface —
/// the single source every machine-readable registry publication stamps.
/// Bump MECHANICS (QA R5, da/api-contract): the surface is a contract for
/// second consumers (devai-ui dogfoods it), so the constant moves when
/// the published contract changes shape for them — a new operation joins
/// or leaves the table, or a request/response shape gains/loses a field a
/// consumer reads. It is bumped AT the publication of that increment (in
/// the same change as the operations/shape entries it versions), never
/// lazily later and never for a prose-only edit; grep finds ONE
/// declaration — there is no second copy to drift.
/// `@2` named the cycle that moved the mutation plane onto the surface
/// (REQ-MUT-*). `@3` names the human-signboard increment shipped with it:
/// the surface gained the rename/advance name doors (`task.rename`, the
/// spec-advance signboard stamp) and the read shapes now carry the name
/// plane (`Task.human_name`, `TaskDependency.task_display`/
/// `depends_on_display`) — a consumer typed against `@2` reads those
/// fields as unknown-absent, so the stamp must move with them.
pub const SURFACE_VERSION: &str = "api-surface@3";

/// The published registry as ONE machine-readable artifact: the surface
/// version beside the full operation table. `devai api registry --json`
/// renders exactly this (R3: the version lands in the registry OUTPUT;
/// no separate dump file is maintained — the registry IS the artifact).
/// Output-only by shape: the descriptors carry `&'static str`, so the
/// machine contract is the RENDERED JSON, not an ingestion type.
#[derive(Serialize, Clone, Debug)]
pub struct RegistryPublication {
    pub surface_version: String,
    pub operations: Vec<OpDescriptor>,
}

/// The publication assembled from the single-source constant and the
/// single-source operation table — no consumer may restamp either.
pub fn publication() -> RegistryPublication {
    RegistryPublication {
        surface_version: SURFACE_VERSION.to_string(),
        operations: operations(),
    }
}

/// The full registry of operations the API surface publishes. Cycle 1 ships
/// the read plane as working code; every other entry names the operation
/// whose implementation lands with the mutation plane (cycle 2).
pub fn operations() -> Vec<OpDescriptor> {
    // NOTE: kept as one flat const-style table — the registry IS data here,
    // and `devai api registry --json` renders it verbatim.
    vec![
        // ---- project ----
        OpDescriptor {
            name: "project.add",
            class: OpClass::Mutation,
            input: "ProjectAddReq{name, description, repos}",
            output: "Project",
            description: "Register a project with optional local repository paths; inserts one projects row. No workflow state involved.",
        },
        OpDescriptor {
            name: "project.list",
            class: OpClass::Read,
            input: "()",
            output: "Vec<Project>",
            description: "List all registered projects ordered by name.",
        },
        OpDescriptor {
            name: "project.get",
            class: OpClass::Read,
            input: "id_or_name",
            output: "Option<Project>",
            description: "Look up one project by full id or name; None when absent.",
        },
        // ---- task: creation & reads ----
        OpDescriptor {
            name: "task.create",
            class: OpClass::Mutation,
            input: "TaskCreateReq{title, project_id, description, parent, blocked_by, idempotency_key}",
            output: "TaskCreated{task, linked}",
            description: "Create a task in DRAFT and register 'parent'/'blocks' dependency edges at the moment of creation (REQ-DEP-07); dangling references fail loudly. The project name resolves through the `project.get` read, so its not-found refusal keeps the exact position the command had. A NULL `project_id` means unbound: the CLI applies the ratified OQ-BIND-1 default BEFORE this operation (working directory matched against the projects' repos — explicit `--project` always wins), the operation itself stays neutral.",
        },
        OpDescriptor {
            name: "task.list",
            class: OpClass::Read,
            input: "()",
            output: "Vec<Task>",
            description: "List all tasks ordered by most recently updated.",
        },
        OpDescriptor {
            name: "task.get",
            class: OpClass::Read,
            input: "id_prefix",
            output: "Task",
            description: "Resolve one task by full id or unique id prefix; fails with not_found/ambiguous. Mirrors `task get`.",
        },
        OpDescriptor {
            name: "task.status",
            class: OpClass::Read,
            input: "id_prefix",
            output: "StatusView{task, allowed_operations, current_artifacts, open_questions}",
            description: "Current state plus the operations the platform permits next, the CURRENT artifacts and open questions. Mirrors `task status`.",
        },
        OpDescriptor {
            name: "task.progress",
            class: OpClass::Read,
            input: "id_prefix",
            output: "ProgressView{task, snapshot_at, pipeline, allowed_operations, running_executions}",
            description: "Pipeline position, re-entry note and the snapshot of RUNNING executions, drawn purely from engine data. Mirrors `task progress`.",
        },
        OpDescriptor {
            name: "task.active",
            class: OpClass::Read,
            input: "()",
            output: "ActiveSnapshot{snapshot_at, rows}",
            description: "Cross-task read-only snapshot of every RUNNING execution with liveness triage (alive/dead/unknown) and worktree link only where the platform recorded worktree.prepared. Mirrors `task active`.",
        },
        OpDescriptor {
            name: "task.workspace_changes",
            class: OpClass::Read,
            input: "id_prefix",
            output: "WorkspaceChangesView{task, state, isolated, zero_changes, partial, branch, worktree, bound_head, tip, head_before, head_after, commits, files, note}",
            description: "The committed changeset of the task's isolated workspace: file list with A/M/D status markers between the stored bound baseline (the bound_head of the first worktree.prepared) and the branch tip — or, once the branch is gone, between the baseline and the last stored worktree.committed stamp when those shas still resolve (state 'historical', partial marker). Verdicts in data: no_isolation / live / historical / unavailable; a zero-change run is an empty list with the zero_changes marker, never an error. Read-only git plumbing over stored facts; nothing is written. Mirrors `task workspace`.",
        },
        OpDescriptor {
            name: "task.workspace_diff",
            class: OpClass::Read,
            input: "id_prefix, path",
            output: "WorkspaceDiffView{task, path, state, branch, baseline, tip, binary, size_bytes, diff, note}",
            description: "Readable unified diff of ONE changeset-listed file of the task's rework branch between exactly the baseline and tip task.workspace_changes used (same resolution, so list and diff are consistent by construction). A path outside the changeset is a typed not_found the consumer branches on by code; binary content is a binary fact with the stored size — raw bytes are never rendered. Mirrors `task workspace-diff`.",
        },
        OpDescriptor {
            name: "task.report",
            class: OpClass::Read,
            input: "id_prefix",
            output: "QualityReportView",
            description: "The quality report shown at the QA gate: the CURRENT report, or — when the task was returned to rework and no re-audit has published yet — the STALE predecessor under an explicit freshness marker (never passed off as the decision). Artifact identity, contract, machine body and the stored human rendering. Mirrors `task report`.",
        },
        OpDescriptor {
            name: "task.events",
            class: OpClass::Read,
            input: "id_prefix, limit",
            output: "Vec<WorkflowEvent>",
            description: "Immutable event history of the task, newest tail limited. Mirrors `task events`.",
        },
        OpDescriptor {
            name: "task.decisions",
            class: OpClass::Read,
            input: "id_prefix",
            output: "Vec<Decision>",
            description: "Decision rows recorded for the task (approvals, overrides, ratifications). Backs APPROVED_DECISIONS delivery to roles; the CLI exposes it as `task decisions`.",
        },
        OpDescriptor {
            name: "task.dependencies",
            class: OpClass::Read,
            input: "id_prefix",
            output: "DependenciesView{task, outgoing, incoming, epic_children, own_parents}",
            description: "Typed dependency view of one task (outgoing/incoming ACTIVE edges); the epic vertex role reads out of the same data. Mirrors `task dependencies`.",
        },
        OpDescriptor {
            name: "task.epics",
            class: OpClass::Read,
            input: "()",
            output: "Vec<EpicVertex>",
            description: "Epic registry as a VIEW over the dependency graph (no epic entity exists). Mirrors `task epics`.",
        },
        OpDescriptor {
            name: "task.graph",
            class: OpClass::Read,
            input: "()",
            output: "GraphView{nodes, ready}",
            description: "The task dependency graph with states and the ready set (no open blockers, non-terminal), built purely from stored data. Mirrors `task graph`.",
        },
        // ---- task: workflow mutations (engine commands; working code in
        // crates/devai-api/src/mutations.rs — the dispatch `task_op` and
        // every named operation are the same path, REQ-MUT-05 AC3) ----
        OpDescriptor {
            name: "task.start",
            class: OpClass::Mutation,
            input: "id_prefix",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "DRAFT → ANALYSIS through workflow::start. Mirrors `task start`.",
        },
        OpDescriptor {
            name: "task.run",
            class: OpClass::Mutation,
            input: "RunReq{id, agent, workspace, force_reaudit}",
            output: "RunOutcome{report, current_quality_report, gate_report_text, gate_report_freshness, gate_human_paragraphs, allowed_operations}",
            description: "Drive assigned roles through all autonomous stages to the next human gate; refuses while a RUNNING execution exists (single writer). force_reaudit carries the --force-reaudit gate semantics. Mirrors `task run`.",
        },
        OpDescriptor {
            name: "task.revise",
            class: OpClass::Mutation,
            input: "ReviseReq{id, name, content, reason}",
            output: "ReviseOutcome{task, staled_ids, allowed_operations}",
            description: "Publish/replace the REQUIREMENTS_SPEC and move to REVIEW through the contract gate. Mirrors `task revise`.",
        },
        OpDescriptor {
            name: "task.review",
            class: OpClass::Mutation,
            input: "id_prefix, verdict, summary",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Record the reviewer verdict in REVIEW. Mirrors `task review`.",
        },
        OpDescriptor {
            name: "task.approve",
            class: OpClass::Mutation,
            input: "id_prefix, rationale",
            output: "ApproveOutcome{mutation: MutationResult, decision: Decision}",
            description: "APPROVAL → IMPLEMENTATION: requires a CURRENT REQUIREMENTS_SPEC; persists the workflow event AND a requirements_approval decision row. Mirrors `task approve`.",
        },
        OpDescriptor {
            name: "task.reject",
            class: OpClass::Mutation,
            input: "id_prefix, rationale",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "APPROVAL → ANALYSIS. Mirrors `task reject`.",
        },
        OpDescriptor {
            name: "task.accept",
            class: OpClass::Mutation,
            input: "id_prefix, rationale",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "QA → ACCEPTED; refuses while open 'blocks' dependencies stand unless a dependency_override decision covers every blocker. Mirrors `task accept`.",
        },
        OpDescriptor {
            name: "task.return",
            class: OpClass::Mutation,
            input: "id_prefix, rationale",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "QA → REWORK. Mirrors `task return`.",
        },
        OpDescriptor {
            name: "task.pause",
            class: OpClass::Mutation,
            input: "id_prefix, reason",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Park the task in PAUSED. Mirrors `task pause`.",
        },
        OpDescriptor {
            name: "task.resume",
            class: OpClass::Mutation,
            input: "id_prefix",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Resume a PAUSED/BLOCKED/FAILED task or answer-driven restore, subject to the dependency gate. Mirrors `task resume`.",
        },
        OpDescriptor {
            name: "task.block",
            class: OpClass::Mutation,
            input: "id_prefix, reason",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Move the task to BLOCKED. Mirrors `task block`.",
        },
        OpDescriptor {
            name: "task.cancel",
            class: OpClass::Mutation,
            input: "id_prefix, reason",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Terminal CANCELLED. Mirrors `task cancel`.",
        },
        OpDescriptor {
            name: "task.fail",
            class: OpClass::Mutation,
            input: "id_prefix, reason",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Record a runtime failure; the task parks in FAILED for `resume`. Mirrors `task fail`.",
        },
        // ---- task: project binding (C5 D1, REQ-BIND-02/03) ----
        OpDescriptor {
            name: "task.project.set",
            class: OpClass::Mutation,
            input: "TaskProjectSetReq{id, project, expected_lock}",
            output: "TaskProjectSetOutcome{task, changed}",
            description: "Assign or replace the OWNING project of an existing task: refs resolve first (unknown task or project refuses typed `not_found`), a stale `expected_lock` refuses typed `conflict`, re-applying the already-bound project is a side-effect-free NO-OP (`changed=false`, no lock bump, no event), an effective change bumps lock_version and appends one `task.project.set` event in the SAME transaction. Touches no workflow state. Mirrors `task set-project`.",
        },
        OpDescriptor {
            name: "task.project.backfill",
            class: OpClass::Mutation,
            input: "id_or_name",
            output: "TaskProjectBackfillOutcome{bound, already_bound, skipped_terminal, skipped_other_project}",
            description: "REQ-BIND-03 under the ratified OQ-BIND-2 rule: bind the given project to every task that lives UNBOUND and NON-TERMINAL, each through `task.project.set` (one event, one lock bump per task). Terminal history keeps the address it lived and judged at; tasks owned by another project are never re-bound. Re-application finds no candidates and changes nothing (idempotent). Mirrors `task backfill-project`.",
        },
        // ---- task: human signboard (task 01a1ad1d, REQ-TN, OQ-TN-1) ----
        OpDescriptor {
            name: "task.rename",
            class: OpClass::Mutation,
            input: "TaskRenameReq{id, name, expected_lock}",
            output: "TaskRenameOutcome{task, human_name, changed}",
            description: "HUMAN door to the signboard (ratified OQ-TN-1): set or replace the task's human name directly — no rework cycle, the workflow state is never touched. The name is validated against the SAME declarative window as spec publication (config/workflow/task-name.yaml: blank / too-short / too-long / multi-line values refuse typed `contract_violation`); refs resolve first, a stale `expected_lock` refuses typed `conflict`, re-applying the name the task already carries is a side-effect-free NO-OP (`changed=false`), an effective change bumps lock_version and appends one `task.human_name.set` event in the SAME transaction. The formal title is never rewritten (REQ-TN-01). Mirrors `task rename`.",
        },
        OpDescriptor {
            name: "task.depend",
            class: OpClass::Mutation,
            input: "DependReq{id, on, kind}",
            output: "TaskDependency",
            description: "Register a typed ACTIVE dependency edge, loud at write on self-reference, duplicate, dangling or intra-kind cycles. Mirrors `task depend`.",
        },
        OpDescriptor {
            name: "task.unlink",
            class: OpClass::Mutation,
            input: "DependReq{id, on, kind}",
            output: "UnlinkResult{edge, superseded_by}",
            description: "Supersede an ACTIVE edge; the row stays as history immutable (OQ7) and the edge is returned AS IT WAS while ACTIVE — the row's own actor is the original author, the superseder is `superseded_by`. Mirrors `task unlink`.",
        },
        // ---- human-only gate escapes (RATIFIED OQ-13: v1 privilege of the
        // human operator — published so a consumer sees them, enforced by the
        // actor rights of the calling session, never by the transport) ----
        OpDescriptor {
            name: "task.override",
            class: OpClass::Mutation,
            input: "id_prefix, blockers, rationale",
            output: "OverrideResult{task, override_blockers}",
            description: "HUMAN-ONLY (OQ-13): record a dependency_override ratification to continue despite open blockers; refs bound to exactly the covered ids. Mirrors `task override`.",
        },
        OpDescriptor {
            name: "task.ratify_empty_rework",
            class: OpClass::Mutation,
            input: "id_prefix, rationale",
            output: "RatifyResult{task, decision, recorded_now, prev_report, current_report, state, allowed_operations}",
            description: "HUMAN-ONLY (OQ-13): the EMPTY-REWORK GATE escape — records rework_noop_ratified for the exact report pair, releases the gate question, runs the withheld REWORK → QA transition. Mirrors `task ratify-empty-rework`.",
        },
        OpDescriptor {
            name: "task.merge",
            class: OpClass::Mutation,
            input: "MergeRecordReq{id, branch, workspace, head_before, head_after, rationale}",
            output: "MergeResult{task_id, branch, workspace, head_before, head_after}",
            description: "HUMAN-ONLY (OQ-13): the persistence half of the single merge path — records the `rework_merge` decision and `rework.merged` event for a merge of the platform rework/<task-id> branch into the checked-out baseline. The repository act itself stays in the human-invoked command (REQ-WT-07 keeps the git invocation behind the CLI subcommand so no engine path reaches it); this operation never touches task state. Mirrors the recording inside `task merge`.",
        },
        // ---- artifacts ----
        OpDescriptor {
            name: "artifact.publish",
            class: OpClass::Mutation,
            input: "PublishReq{id, kind, name, content, depends, contract, execution, verdict, category}",
            output: "PublishOutcome{publish: PublishedRow{artifact, task}, advanced: Option<AdvanceOutcome>}",
            description: "Publish a versioned artifact through the contract gate; certain kinds advance the workflow through the SAME core commands. Requires a RUNNING execution row when execution is given. Mirrors `artifact publish`.",
        },
        OpDescriptor {
            name: "artifact.publish_row",
            class: OpClass::Mutation,
            input: "PublishReq{id, kind, name, content, depends, contract, execution, verdict, category}",
            output: "PublishedRow{artifact, task}",
            description: "The first phase of `artifact.publish` alone: the row through the contract gate, no workflow advance. Exists beside the compound so a consumer that renders between the phases (the CLI, printing the artifact row before the advance) keeps output parity even when the advance fails.",
        },
        OpDescriptor {
            name: "artifact.advance",
            class: OpClass::Mutation,
            input: "AdvanceReq{id, artifact_id, kind, verdict}",
            output: "AdvanceOutcome{from, to, rework_gate}",
            description: "The second phase of `artifact.publish`: the workflow advance certain kinds carry (requirements → REVIEW, review report → gate, implementation report → QA through the empty-rework gate). Same core commands as the compound and the engine.",
        },
        OpDescriptor {
            name: "artifact.list",
            class: OpClass::Read,
            input: "id_prefix",
            output: "Vec<Artifact>",
            description: "All artifact rows of the task with versions and statuses. Mirrors `artifact list`.",
        },
        OpDescriptor {
            name: "artifact.cat",
            class: OpClass::Read,
            input: "artifact_id",
            output: "Vec<u8> (raw bytes)",
            description: "Fetch the stored content bytes of one artifact. Mirrors `artifact cat`.",
        },
        OpDescriptor {
            name: "artifact.cat_text",
            class: OpClass::Read,
            input: "artifact_id",
            output: "String",
            description: "Text presentation of artifact content; NotUtf8 error when the bytes are not text.",
        },
        // ---- human-wait section (task 01a1b0ea) ----
        OpDescriptor {
            name: "inbox.snapshot",
            class: OpClass::Read,
            input: "()",
            output: "InboxSnapshotView{snapshot_at, policy, tasks: Vec<InboxTaskFacts{task, questions, spec, report, report_note}>}",
            description: "The human-wait section in ONE batch read: the declarative demand table of the home (config/workflow/inbox-waits.yaml) plus every task's stored questions and its gate presentation (CURRENT/LAST REQUIREMENTS_SPEC body for APPROVAL, CURRENT quality report for QA). One snapshot per data type instead of a per-task fan-out; membership is decided by the caller on exact stored tokens.",
        },
        // ---- questions ----
        OpDescriptor {
            name: "question.ask",
            class: OpClass::Mutation,
            input: "AskReq{id, text, context, options, blocking, execution}",
            output: "QuestionAsked{question, parked}",
            description: "Raise a question; a blocking one parks the task in WAITING_FOR_INPUT through workflow::raise_question. Mirrors `question ask`.",
        },
        OpDescriptor {
            name: "question.answer",
            class: OpClass::Mutation,
            input: "question_id, text",
            output: "AnsweredOutcome{question_id, from_state, to_state}",
            description: "Answer an OPEN question and run the answer-driven transition. Mirrors `question answer`.",
        },
        OpDescriptor {
            name: "question.list",
            class: OpClass::Read,
            input: "id_prefix",
            output: "Vec<Question>",
            description: "All questions of the task. Mirrors `question list`.",
        },
        // ---- roles / agents / contracts ----
        OpDescriptor {
            name: "role.add",
            class: OpClass::Mutation,
            input: "file (RoleDefinition JSON)",
            output: "RoleDefinition",
            description: "Register a versioned RoleDefinition; version 0 in the file means latest+1 for the slug. Mirrors `role add`.",
        },
        OpDescriptor {
            name: "role.list",
            class: OpClass::Read,
            input: "()",
            output: "Vec<RoleDefinition>",
            description: "List all registered role versions.",
        },
        OpDescriptor {
            name: "role.get",
            class: OpClass::Read,
            input: "slug",
            output: "Option<RoleDefinition>",
            description: "Latest definition for a role slug.",
        },
        OpDescriptor {
            name: "agent.add",
            class: OpClass::Mutation,
            input: "name, provider, model, runtime",
            output: "AgentInstance",
            description: "Register a local agent instance; credentials stay outside the store. Mirrors `agent add`.",
        },
        OpDescriptor {
            name: "agent.list",
            class: OpClass::Read,
            input: "()",
            output: "Vec<AgentInstance>",
            description: "List registered agents.",
        },
        OpDescriptor {
            name: "contract.add",
            class: OpClass::Mutation,
            input: "ContractAddReq{name, version, schema}",
            output: "ContractRegistration{name, version}",
            description: "Register a versioned contract schema. Mirrors `contract add`.",
        },
        OpDescriptor {
            name: "contract.get",
            class: OpClass::Read,
            input: "name, version",
            output: "Option<String> (schema JSON text)",
            description: "Fetch a stored contract schema. Mirrors `contract get`.",
        },
        // ---- executions ----
        OpDescriptor {
            name: "exec.start",
            class: OpClass::Mutation,
            input: "ExecStartReq{id, role, agent, inputs}",
            output: "ExecStarted{execution, task}",
            description: "Start a role execution: role/stage matrix validated, workspace revision and owner columns captured platform-side, single-writer guard enforced. Mirrors `exec start`.",
        },
        OpDescriptor {
            name: "exec.finish",
            class: OpClass::Mutation,
            input: "exec_id, summary",
            output: "RoleExecution",
            description: "Close a RUNNING row as COMPLETED. Mirrors `exec finish`.",
        },
        OpDescriptor {
            name: "exec.fail",
            class: OpClass::Mutation,
            input: "exec_id, error",
            output: "RoleExecution",
            description: "Close a RUNNING row as FAILED with the error text. Mirrors `exec fail`.",
        },
        OpDescriptor {
            name: "exec.cancel",
            class: OpClass::Mutation,
            input: "exec_id",
            output: "RoleExecution",
            description: "Close a RUNNING row as CANCELLED (cooperative; the process is never touched, REQ-SW-06). Mirrors `exec cancel`.",
        },
        OpDescriptor {
            name: "exec.list",
            class: OpClass::Read,
            input: "Option<task_ref>",
            output: "Vec<RoleExecution>",
            description: "Execution rows, optionally for one task; attempt/failure_class lineage ships in the rows. Mirrors `exec list`.",
        },
        OpDescriptor {
            name: "exec.get",
            class: OpClass::Read,
            input: "exec_id_prefix",
            output: "RoleExecution",
            description: "One execution row by full id or unique prefix — machine observation of a running process's state without the launching terminal (REQ-API-07). Backs `exec get`.",
        },
        OpDescriptor {
            name: "exec.liveness",
            class: OpClass::Read,
            input: "exec_id_prefix",
            output: "LivenessView{id, status, owner_pid, owner_host, liveness}",
            description: "Liveness verdict (alive/dead/unknown) for the recorded owner of one execution — orphans distinguishable by data, not pgrep. Backs `exec liveness`.",
        },
        OpDescriptor {
            name: "exec.active_rows",
            class: OpClass::Read,
            input: "()",
            output: "Vec<ActiveRow>",
            description: "RUNNING rows with per-row liveness, ordered oldest-first — the data behind `task active`; the task plane wraps it into ActiveSnapshot.",
        },
        // ---- export / import ----
        OpDescriptor {
            name: "export.bundle",
            class: OpClass::Read,
            input: "id_prefix, out_dir",
            output: "BundleManifest",
            description: "Export the task bundle (domain data + artifacts, no caches/credentials) — identity and versions identical to the CLI path (REQ-API-02 AC3). Mirrors `export`.",
        },
        OpDescriptor {
            name: "import.bundle",
            class: OpClass::Mutation,
            input: "dir",
            output: "ImportOutcome",
            description: "Import a bundle, respecting single-CURRENT and single-writer invariants. Mirrors `import`.",
        },
        // ---- registry ----
        OpDescriptor {
            name: "api.registry",
            class: OpClass::Read,
            input: "()",
            output: "Vec<OpDescriptor>",
            description: "The published operation registry itself — every operation with typed input/output names and textual semantics. Backs `devai api registry`.",
        },
        // ---- excluded units (RATIFIED OQ-10, encapsulated per human-ratified
        // cycle-2 Q2) ----
        // init/seed stay the bootstrap pair and are NOT operations of this
        // registry — but the handle moved inside the API: consumers open a
        // home through `Api::open` and seed through `Api::seed_config`,
        // never through a returned Store (the cycle-2 prohibition is total).
        // Implemented in crates/devai-cli/src/commands.rs over the API.
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_names_are_unique_and_area_prefixed() {
        let ops = operations();
        let mut seen: Vec<&str> = ops.iter().map(|o| o.name).collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            total,
            "duplicate operation names in the registry"
        );
        for o in operations() {
            assert!(o.name.contains('.'), "{}: name must be area.action", o.name);
        }
    }

    #[test]
    fn every_operation_carries_textual_semantics() {
        // REQ-API-01 AC2 / REQ-API-06 AC1: a description is stored data,
        // never a bare code reference.
        for o in operations() {
            assert!(
                o.description.trim().len() >= 20,
                "{}: description too thin",
                o.name
            );
            assert!(
                !o.input.is_empty() && !o.output.is_empty(),
                "{}: shapes must be named",
                o.name
            );
        }
    }

    #[test]
    fn registry_serializes_as_machine_readable_json() {
        // REQ-API-06 AC4: the published artifact shape.
        let v: serde_json::Value = serde_json::to_value(operations()).unwrap();
        assert_eq!(v.as_array().unwrap().len(), operations().len());
        assert_eq!(v[0]["class"].as_str().unwrap(), "mutation"); // lowercase tokens
        assert!(v[0]["name"].is_string() && v[0]["description"].is_string());
    }

    #[test]
    fn publication_stamps_the_single_source_surface_version() {
        // R3 QA (da/api-contract) parity: the machine-readable registry
        // publication CARRIES the declared version, and the constant is
        // its only source — a registry output without the version, or a
        // version restated anywhere else in the published document,
        // fails here.
        let v: serde_json::Value = serde_json::to_value(publication()).unwrap();
        assert_eq!(
            v["surface_version"].as_str().unwrap(),
            SURFACE_VERSION,
            "the publication names the declared surface version"
        );
        assert_eq!(
            v["operations"].as_array().unwrap().len(),
            operations().len(),
            "the versioned publication wraps the FULL operation table"
        );
        assert!(
            !SURFACE_VERSION.is_empty(),
            "the version constant must be a real contract stamp"
        );
        // The RENDERED document is machine-readable for second consumers:
        // serialize → re-parse shows both contract fields as data.
        let text = serde_json::to_string(&publication()).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["surface_version"].as_str().unwrap(), SURFACE_VERSION);
        assert!(doc["operations"].is_array());
    }

    #[test]
    fn surface_version_v3_ships_the_name_plane() {
        // QA R5 (da/api-contract): the stamp and its increment are
        // pinned TOGETHER — @3 is by this test defined as "the
        // human-signboard surface": if the constant moves without the
        // rename door/shape moving with it, or the name plane leaves the
        // table while the stamp claims it, this fails. The literal is
        // the deliberate second writing of the version: bumping the
        // constant is an author's decision documented at the declaration
        // (see the bump mechanics comment there), and this test forces
        // that decision to be taken consciously, twice, in one change.
        assert_eq!(SURFACE_VERSION, "api-surface@3");
        let rename = operations()
            .into_iter()
            .find(|o| o.name == "task.rename")
            .expect("@3 surface must publish task.rename (ratified OQ-TN-1)");
        assert_eq!(rename.class, OpClass::Mutation);
        assert!(
            rename.output.contains("human_name"),
            "the rename outcome shape must carry the signboard: {}",
            rename.output
        );
    }

    #[test]
    fn init_seed_exception_is_not_an_operation() {
        // RATIFIED OQ-10 (encapsulated per cycle-2 Q2): the bootstrap pair
        // is not an operation of the registry in either class — the API
        // owns the handle (`Api::open`/`seed_config`), consumers never do.
        for o in operations() {
            assert!(
                !o.name.starts_with("init") && !o.name.starts_with("seed"),
                "{}: OQ-10 exception leaked into the registry",
                o.name
            );
        }
    }

    #[test]
    fn mutation_plane_is_fully_registered() {
        // REQ-MUT-05: every mutation the cycle-2 surface implements has a
        // registry entry of class Mutation. The list mirrors the public
        // operations in crates/devai-api/src/mutations.rs.
        let muts: std::collections::HashSet<&str> = operations()
            .iter()
            .filter(|o| o.class == OpClass::Mutation)
            .map(|o| o.name)
            .collect();
        for n in [
            "project.add",
            "task.create",
            "task.start",
            "task.run",
            "task.revise",
            "task.review",
            "task.approve",
            "task.reject",
            "task.accept",
            "task.return",
            "task.pause",
            "task.resume",
            "task.block",
            "task.cancel",
            "task.fail",
            "task.project.set",
            "task.project.backfill",
            "task.depend",
            "task.unlink",
            "task.override",
            "task.ratify_empty_rework",
            "task.merge",
            "artifact.publish",
            "artifact.publish_row",
            "artifact.advance",
            "question.ask",
            "question.answer",
            "role.add",
            "agent.add",
            "contract.add",
            "exec.start",
            "exec.finish",
            "exec.fail",
            "exec.cancel",
            "import.bundle",
        ] {
            assert!(
                muts.contains(n),
                "{n}: mutation of the API surface missing from the registry"
            );
        }
    }
}
