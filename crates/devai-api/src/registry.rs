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
/// `Registry` is the platform plane introduced by the homes-registry cycle
/// (REQ-HOME-05: reading domain records and managing the registry are NOT
/// mixed in one operation): a Registry operation's single writable target
/// is the homes-registry file of the control home — it never touches any
/// home's domain rows, which stay governed by Read (UI) and Mutation
/// (CLI/engine) exactly as before.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OpClass {
    Read,
    Mutation,
    Registry,
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
/// (REQ-MUT-*). `@3` named the human-signboard increment: the surface
/// gained the rename/advance name doors (`task.rename`, the spec-advance
/// signboard stamp) and the read shapes now carry the name plane
/// (`Task.human_name`, `TaskDependency.task_display`/
/// `depends_on_display`). `@4` names the homes-registry increment built
/// ON TOP of that main surface: the operation vocabulary grew a class
/// token (`registry`) and the `home.*` plane (list/projects_all/tasks_all
/// plus the add/set_enabled/activate management doors) — additive for
/// tolerant readers, but wire-visible, so the version says so (a
/// vocabulary change never travels without the stamp). `@5` names the
/// project-rules ratification increment (task 01a1bacf): `profile.state` /
/// `profile.ratify` / `profile.revoke` join the table, and `Decision` gains
/// the `payload` field a ratification's `<project, edition, items>` binding
/// is read from — a wire-visible shape change for second consumers.
/// `@6` names the model-bundle increment (task 01a1b71f): the surface
/// gained `model.snapshot` / `model.probe` / `model.switch` and the run
/// stamp (`RoleExecution.bundle`) second consumers read — wire-visible, so
/// the version says so.
/// `@7` names the bundle-authoring increment (task 01a1be5c): the owner's
/// form gained `model.bundle_check` / `model.bundle_create` /
/// `model.bundle_update` / `model.bundle_delete`, `ModelSnapshot` gained
/// `file_revision` + `form`, and `ModelEventView` gained `outcome_text` —
/// wire-visible additions, so the stamp says so.
/// `@8` names the single-source follow-up of the same task: the form no
/// longer derives a handle itself — `model.bundle_handle` is the ONE read
/// that answers «what id will the save write?», so the window's copy of the
/// rule could be deleted rather than kept in sync. Wire-visible addition
/// (one Read operation), so the stamp says so.
/// `@9` named the convergence increment (task 01a1b5be, REQ-CV-14): NO
/// operation joined or left the table — the slice rode on `task.progress` and
/// `inbox.snapshot`, whose output shapes gained `convergence`.

/// `@10` names the quality-gate increment (task 01a1bfa0, REQ-QG-01..09): NO
/// operation joins or leaves the table — the gate's availability rides on
/// `task.status` (`quality_gate`), a live audit is named on `task.active`
/// rows (`note`), and `inbox.snapshot` facts gain `audit_running` so a running
/// audit is not presented as a human wait. Wire-visible additions, so the
/// stamp says so.
///
/// `@11` names the agent-failure increment (task 01a1b60c, REQ-AF-01..10):
/// the surface gains `agent.status` (the derived availability/class line),
/// `task.progress` and `inbox.snapshot` facts gain `agent_failure`, and the
/// agent-failure vocabulary (`agent.failed` events and their payload) is
/// published as data. Wire-visible additions, so the stamp says so.
///
/// `@12` names the liveness indicator increment (task 01a1b75d, REQ-LV-01/02/
/// 05/06): NO operation joins or leaves the table — `exec.list` rows gain
/// `liveness`/`liveness_line` (present only on an UNFINISHED row),
/// `task.status` gains `running_executions` (the same typed rows
/// `task.progress` already carried, now with owner+liveness facts), and
/// `exec.liveness` gains the platform's sentence `line`.
///
/// `@13` (same task, rework R2): `task.run` gains `line` — the platform's ONE
/// sentence for its typed outcome, so the card's «перезапустить» act prints
/// the platform's words instead of re-wording `started|already_running|
/// refused`. A wire-visible addition, so the stamp says so.
///
/// `@14` names the task-path + names-first increment (task 01a1b5e1,
/// REQ-RG-01..06 / REQ-NM-01): NO operation joins or leaves the table —
/// `task.progress` gains the folded `graph` (stages, role/gate nodes, edges,
/// the ONE position, the rework loop, running roles, human waits, blocking
/// count, notes), `task.active` rows gain `task_display`/`role_display`,
/// `task.status` artifacts and `artifact.list` rows publish the finished
/// `display` label, `exec.list` rows gain `role_display`, and the
/// APPROVAL/QA presentations gain `label`. Wire-visible additions, so the
/// stamp says so.
/// @17: `source.slice` отдаёт `candidates`, когда сокращённый путь совпал с
/// несколькими файлами — выбор вместо догадки. Только чтение.
pub const SURFACE_VERSION: &str = "api-surface@17";

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
            output: "StatusView{task, allowed_operations, current_artifacts: Vec<ArtifactView{…Artifact, display}>, open_questions, quality_gate, running_executions, stop, launch_failure}",
            description: "Current state plus the operations the platform permits next, the CURRENT artifacts, open questions, the RUNNING executions with their liveness fact and the platform's sentence for it (checked at every read), and — at the QA gate — each action's public availability and platform refusal. Mirrors `task status`.",
        },
        OpDescriptor {
            name: "task.progress",
            class: OpClass::Read,
            input: "id_prefix",
            output: "ProgressView{task, snapshot_at, pipeline, graph, allowed_operations, running_executions, convergence, agent_failure}",
            description: "Pipeline position, re-entry note, the whole task PATH folded once from stored facts (`graph`: declared stage axis with positional statuses, role nodes only for executed roles, human gates born from the declaration, edges carrying one artifact/decision/event, the ONE position, the rework loop with round count, running roles, human waits with requirement/hint/presentation, open blocking-question count and notes), the snapshot of RUNNING executions (each with its owner, liveness fact and the platform's sentence) and the convergence slice of the task (per repeatable stage the rounds read out of stored carriers, their computed modes and honest gaps; null on terminal tasks). Drawn purely from engine data. Mirrors `task progress`.",
        },
        OpDescriptor {
            name: "task.active",
            class: OpClass::Read,
            input: "()",
            output: "ActiveSnapshot{snapshot_at, rows: Vec<ActiveRow{…, task_display, role_display, note}>}",
            description: "Cross-task read-only snapshot of every RUNNING execution with liveness triage (alive/dead/unknown), worktree link only where the platform recorded worktree.prepared, and the platform's note for a live quality audit («идёт аудит»). Mirrors `task active`.",
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
            name: "task.workspace_diff_full",
            class: OpClass::Read,
            input: "id_prefix, path",
            output: "WorkspaceDiffView{task, path, state, branch, baseline, tip, binary, size_bytes, diff, note}",
            description: "The same single-file diff as task.workspace_diff over the same baseline..tip range and changeset check, printed with the WHOLE file as unchanged context (`git diff -U<whole>`). Exists so a reader can unfold the lines the compact diff hides without a second resolution and without recomputing git: range, verdicts and binary handling are identical by construction. Mirrors `task workspace-diff --full`.",
        },
        OpDescriptor {
            name: "task.signature",
            class: OpClass::Read,
            input: "id_prefix",
            output: "SignatureView{task_id,state,snapshot_at,may_approve,may_approve_denied,may_reject,may_reject_denied,spec,review,review_note,previous,previous_note,signature}",
            description: "The APPROVAL card's signable surface, from stored facts only: the presented REQUIREMENTS_SPEC (CURRENT, else the last one with current=false) with its human paragraph, requirements (id/statement/acceptance_criteria/constraints) and document open_questions; the reviewer block (verdict as stored, human paragraph or its summary, honest note per missing/unreadable branch); the previous spec version with a human field diff that also states, per side, whether that side carries its own paragraph at all (a readable pre-protocol version has none — the business-description fallback is never diffed as one); the two independent signature rights may_approve/may_reject with the first denial code; and the newest stored signature fact (decision row preferred over event, spec_seq only from resolvable refs). One read for the whole card — no per-item artifact reads, no writes, no re-derivation. Mirrors `task signature`.",
        },
        OpDescriptor {
            name: "task.report",
            class: OpClass::Read,
            input: "id_prefix",
            output: "QualityReportView{task, artifact, label, seq, …}",
            description: "The quality report shown at the QA gate: the CURRENT report, or — when the task was returned to rework and no re-audit has published yet — the STALE predecessor under an explicit freshness marker (never passed off as the decision). Artifact identity, the finished «название вида · vN» heading, contract, machine body and the stored human rendering. Mirrors `task report`.",
        },
        OpDescriptor {
            name: "task.usage",
            class: OpClass::Read,
            input: "id_prefix",
            output: "TaskUsageView{task,total,cost,runs,measured,rows}",
            description: "Расход токенов задачи по прогонам: драйвер пишет usage.json в каталог улик каждого прогона, операция читает эти файлы и агрегирует. Честно отдаёт runs/measured — прогоны без отчёта считаются, но в сумму не входят.",
        },
        OpDescriptor {
            name: "eval.rules",
            class: OpClass::Read,
            input: "—",
            output: "RulebookView{profile_version,project,theta,floors,blocked_max,groups}",
            description: "Правила оценки дома как данные: группы, вес каждого пункта, шкала 1.0/0.7/0.4/0.0, полы и типы ошибок — из профиля оценки (config/evaluation). Это то, чем судит качество-аудитор; окно показывает правила рядом с результатом, ничего не пересчитывая.",
        },
        OpDescriptor {
            name: "source.slice",
            class: OpClass::Read,
            input: "id_prefix, rev, path, from, to",
            output: "SourceSliceView{path,rev,from,to,total_lines,text}",
            description: "Кусок исходника на зафиксированном коммите (git show <rev>:<path>) по ссылке «путь:строки» из отчёта: окно показывает ровно тот код, о котором говорит проверка.",
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
            output: "RunStartOutcome{run_id, execution_id, outcome(started|already_running|refused), reason, state, line}",
            description: "ASYNC launch (REQ-AR-04): starts a detached platform runner in its own session, returns the run number and the execution handle as soon as the runner opened the row — never waits for a role, never leaves the role in the caller's process group. A second launch is the typed `already_running` outcome naming the execution that owns the task (REQ-AR-06); a human-executed state is `refused` with a reason. `line` is the platform's one sentence for the outcome (a window prints it verbatim). force_reaudit carries the --force-reaudit gate semantics. Mirrors `devai task run`.",
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
            description: "APPROVAL → ANALYSIS: persists the workflow event AND a requirements_rejection decision row (REJECTED, actor, rationale, refs to the CURRENT REQUIREMENTS_SPEC when one exists; refs empty otherwise). Rights are the state alone — a missing or non-CURRENT spec does not narrow reject. Mirrors `task reject`; the window's `task_reject` calls this same operation.",
        },
        OpDescriptor {
            name: "task.rewind",
            class: OpClass::Mutation,
            input: "id_prefix, to, reason",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Человек проходит путь заново с названной стадии: адрес перехода берётся из объявления процесса (rewind_to_*), результаты этой стадии и всех следующих помечаются STALE (ничего не удаляется), причина обязательна и ложится в журнал. Только человек; принятую задачу перепройти нельзя. Mirrors `task rewind`.",
        },
        OpDescriptor {
            name: "task.restate",
            class: OpClass::Mutation,
            input: "id_prefix, title?, description?, reason",
            output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Человек переписывает изначальную постановку задачи. В DRAFT/STATEMENT меняются только поля; на начатом пути это rewind в STATEMENT (результаты ниже → STALE), чтобы постановщик проверил новую постановку. Причина обязательна. Mirrors `task restate`.",
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
        // ---- project rules ratification (task 01a1bacf, RV1/RV4/RV6/RV9) ----
        OpDescriptor {
            name: "profile.state",
            class: OpClass::Read,
            input: "project_id_or_name",
            output: "ProfileState{project_id, project_name, file, edition, items}",
            description: "READ-ONLY state of a project's rules file: the current EDITION fingerprint and every item's force state (in force with the decision + actor, or out with the RV8 reason). Computes nothing the loader would compute differently: same fingerprint, same force rule. Mirrors `profile status`; needs no role run.",
        },
        OpDescriptor {
            name: "profile.ratify",
            class: OpClass::Mutation,
            input: "ProfileRatifyReq{task, project, items, all, rationale}",
            output: "ProfileRatifyOutcome{task_id, decision, recorded_now, project_id, file, edition, items}",
            description: "HUMAN-ONLY: records the `profile_ratification`/APPROVED decision covering the named items of the CURRENT rules edition (the fingerprint is computed platform-side, never typed), then writes the ratification pointers into the file. Rationale is mandatory; an unknown item refuses; the same edition + item set reuses the stored decision instead of writing a second one. Enforced by the API against the CALLING SESSION (a role/engine run is refused typed `human_only`), not by the transport; no role tool grants it. Mirrors `profile ratify`.",
        },
        OpDescriptor {
            name: "profile.revoke",
            class: OpClass::Mutation,
            input: "ProfileRevokeReq{decision, rationale}",
            output: "ProfileRevokeOutcome{decision, revoked, project_id, file, edition, items}",
            description: "HUMAN-ONLY: takes an APPROVED profile_ratification out of force with a separate decision row citing it. The rules file is not rewritten or deleted; a second revocation of the same row refuses. Same session gate as `profile.ratify` (a role/engine run is refused typed `human_only`). Mirrors `profile revoke`.",
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
            output: "Vec<ArtifactView{…Artifact, display}>",
            description: "All artifact rows of the task with versions and statuses, each carrying its finished human label («название вида · vN» from the declarative table) so a consumer never builds a second heading. Mirrors `artifact list`.",
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
            output: "InboxSnapshotView{snapshot_at, policy, tasks: Vec<InboxTaskFacts{task, questions, spec, report, report_note, audit_running, convergence, agent_failure}>}",
            description: "The human-wait section in ONE batch read: the declarative demand table of the home (config/workflow/inbox-waits.yaml) plus every task's stored questions and its gate presentation (CURRENT/LAST REQUIREMENTS_SPEC body for APPROVAL, CURRENT quality report for QA — each headed by its finished «название вида · vN» label) and the task's convergence slice — the SAME object task.progress answers, null on terminal tasks and on rows outside the wait section. One snapshot per data type instead of a per-task fan-out; membership is decided by the caller on exact stored tokens.",
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
            name: "agent.status",
            class: OpClass::Read,
            input: "()",
            output: "Vec<AgentStatusView{name, driver, llm_provider, model, availability, availability_word, last_class, last_class_word, last_failure_at, last_outcome_at, last_seen_at, failures, series, series_first_at, line}>",
            description: "The derived agent status row (task 01a1b60c, REQ-AF-07): availability from the latest executed outcome (available / unavailable / unknown), the last failure class in human words and the ONE rendered line, the failure counters and the time. No stored status exists — the row folds stored executions and failure windows at read time. Mirrors `devai agent list`.",
        },
        OpDescriptor {
            name: "model.snapshot",
            class: OpClass::Read,
            input: "()",
            output: "ModelSnapshot{snapshot_at, active, status, declared, history, failures, running}",
            description: "The whole model-bundle section in ONE read: the marked agent record (driver, LLM-provider, model, reasoning level), the SINGLE computed status, the bundles declared in config/models.yaml, the probe/switch history, the refusal banner with a live suggestion, and the runs in flight. No network call; no store access outside this operation. Mirrors `devai model list`.",
        },
        OpDescriptor {
            name: "model.probe",
            class: OpClass::Mutation,
            input: "bundle_id",
            output: "ProbeView{bundle_id, verdict, latency_ms, exit_code, output_excerpt, at}",
            description: "One real non-interactive driver call of a DECLARED bundle (claude -p / hermes -z) with the bundle's reasoning level and a hard timeout; records exactly one `model.probe` event. Changes no agent record and no marker; `mock` is not_probeable without starting a process. Mirrors `devai model probe`.",
        },
        OpDescriptor {
            name: "model.switch",
            class: OpClass::Mutation,
            input: "bundle_id, reason (manual|limits|error)",
            output: "SwitchResultView{outcome, bundle_id, verdict, latency_ms, code}",
            description: "ONE platform operation: probe the declared target, and only on `ok` upsert the agent record by name (driver, LLM-provider, model AND reasoning level), move the single active marker, and append `model.switched` — all in one verified transaction (a failure rolls the whole switch back). A dead verdict writes `refused_probe` and changes nothing; unknown id / no_op / not_probeable refuse before any event. Mirrors `devai model switch`.",
        },
        OpDescriptor {
            name: "model.bundle_handle",
            class: OpClass::Read,
            input: "label",
            output: "BundleHandleView{id}",
            description: "The handle a NEW bundle will carry, derived from the label by the store's ONE rule (ASM-9). A pure read: no file read, no event, no write. The window prints this value verbatim instead of mirroring the rule, so the id shown in the form is by construction the id a save writes.",
        },
        OpDescriptor {
            name: "model.bundle_check",
            class: OpClass::Mutation,
            input: "BundleFormInput{label, agent, driver, llm_provider, model, reasoning, probe{phrase, timeout_ms, fresh_secs}, editing, id?}",
            output: "ProbeView{bundle_id, verdict, latency_ms, exit_code, output_excerpt, at}",
            description: "The form's «Проверить»: ONE real probe of the CURRENT form values, one `model.probe` event, and NOTHING written to config/models.yaml. A create draft does not need a declared record; an edit draft is addressed by its id. The save probes again — a passed check never authorizes a save (REQ-MC-02).",
        },
        OpDescriptor {
            name: "model.bundle_create",
            class: OpClass::Mutation,
            input: "BundleFormInput + file_revision",
            output: "BundleMutationView{outcome, bundle_id, label, verdict, latency_ms, agent_changed, revision}",
            description: "Create a declared bundle from the window (REQ-MC-01): validate the fields (model and reasoning level required, `mock` refused, no provider for a self-providing driver, positive probe policy), derive the handle from the name once (ASM-9), refuse a taken handle or a duplicate identity NAMING both records, PROBE the exact values and only on `ok` write config/models.yaml atomically (comments preserved). The bundle does not become active. Every attempt is a `model.bundle_created` fact.",
        },
        OpDescriptor {
            name: "model.bundle_update",
            class: OpClass::Mutation,
            input: "BundleFormInput{id, editing:true, ...} + file_revision",
            output: "BundleMutationView{outcome, bundle_id, label, verdict, latency_ms, agent_changed, revision}",
            description: "Edit a declared bundle (REQ-MC-03): the same probe-before-write path; the handle never changes. Editing the record IN FORCE applies the new identity to the marked agent record after the successful probe (marker stays; a name change marks the new record and leaves the old one unmarked). A stale `file_revision` refuses without touching the file. Every attempt is a `model.bundle_updated` fact.",
        },
        OpDescriptor {
            name: "model.bundle_delete",
            class: OpClass::Mutation,
            input: "bundle_id, file_revision",
            output: "BundleMutationView{outcome, bundle_id, label, revision}",
            description: "Delete one DECLARED bundle from config/models.yaml (REQ-MC-05): atomic and comment-preserving; agent records are untouched and runs in flight are not stopped. The bundle in force refuses with «сначала переключитесь». Every attempt is a `model.bundle_deleted` fact.",
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
            output: "Vec<ExecutionView{…RoleExecution, role_display, liveness, liveness_line}>",
            description: "Execution rows, optionally for one task; attempt/failure_class lineage ships in the rows, each row carries «имя роли · vN» as the human signboard beside the slug, and every UNFINISHED row carries the liveness verdict of its recorded owner plus the platform's sentence for it (a finished row is not checked and carries no mark). Mirrors `exec list`.",
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
            output: "LivenessView{id, status, owner_pid, owner_host, liveness, line}",
            description: "Liveness verdict (alive/dead/unknown) for the recorded owner of one execution, plus the platform's ONE sentence for it — orphans distinguishable by data, not pgrep. Backs `exec liveness`.",
        },
        OpDescriptor {
            name: "exec.reap",
            class: OpClass::Mutation,
            input: "Option<task_ref>",
            output: "Vec<Reclaimed{execution, class, evidence}>",
            description: "Close every RUNNING execution whose owner process is PROVABLY gone (host-restarted, process_vanished, or lost_before_ownership for pre-owner rows) with its failure class and an `execution.lost` event; unprovable rows stay untouched and the slot stays held (REQ-AR-05). Never signals a process. Backs `exec reap`.",
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
        // ---- registry plane (homes registry: platform data, REQ-HOME-01…09).
        // The Registry class is the separated write plane: its single
        // writable target is `homes.json` of the control home. Reading home
        // records and managing the registry are distinct operations and
        // never mix (REQ-HOME-05). ----
        OpDescriptor {
            name: "home.list",
            class: OpClass::Read,
            input: "()",
            output: "Vec<HomeRecord>",
            description: "Enumerate the stored homes registry with attributes: path, project, project_source and project_error (how the project label was derived — store / path / unreadable — plus the read cause when the store could not be read), added_at, enabled, the single stored active marker, plus live-probed available, the home's schema_version and a version_note; read_error carries the typed cause of an unreadable home. The stored document is re-read at each enumeration (a registration committed by another session is visible without a restart); availability is the only read-time probe; activity is a saved fact, never process memory. An absent home renders with its path and cause and stays registered (REQ-HOME-07).",
        },
        OpDescriptor {
            name: "home.add",
            class: OpClass::Registry,
            input: "path",
            output: "HomeRecord",
            description: "Register an existing devai home in the registry: writes the home descriptor into homes.json of the control home ONLY. A missing directory refuses typed invalid_input, a directory without devai.db refuses typed home_no_store — the managed home's content is never copied or migrated by adding it. Re-adding a known path re-enables the record and keeps its original added_at (recent-homes).",
        },
        OpDescriptor {
            name: "home.set_enabled",
            class: OpClass::Registry,
            input: "path, enabled",
            output: "Vec<HomeRecord>",
            description: "Include/exclude a registered home from the merged observation without restarting the process; the registry row itself survives (removal from the registry IS disabling — ratified OQ-HOME-3, physical deletion is out of scope) and the home's data on disk is untouched. Switching the ACTIVE home off is a normal action with a defined outcome (ratified OQ-HOME-8): the next ENABLED record in registry order becomes active (wrapping to the first enabled record when the tail holds none), and when no enabled record remains the registry holds the honest no-active-home state — `home.list` then reports every record with active=false and domain records stay unavailable until the operator enables or adds a home. No home is chosen silently. Returns the fresh records.",
        },
        OpDescriptor {
            name: "home.activate",
            class: OpClass::Registry,
            input: "path",
            output: "Api",
            description: "Store the selected enabled home as the single active home (exactly-one-active flip in homes.json of the control home — the only registry write) and RETURN the reopened observation handle for that home — the act's output is the session's new active API handle, never `()`. The whole transaction — validate the record, re-probe the store on disk, reopen the chosen home as an observation (`Api::open_observed` over the store's read-only open), commit the flip — is one API-plane act: a failed reopen returns before the commit, so the registry can never point at a home the session does not hold (da/architecture-consistency at report v1 moved this orchestration out of the consumer). Activation selects what the session watches; it never opens the target with the migrating bootstrap, so a home of any schema rank keeps its rank and rows untouched (`home.list` stamps the version note). No domain data of any other home is mutated here; activation of a missing/disabled home refuses typed home_unavailable/home_disabled.",
        },
        OpDescriptor {
            name: "home.projects_all",
            class: OpClass::Read,
            input: "Option<home_path>",
            output: "Vec<HomeRecordsView<Project>>",
            description: "Merged projects plane of all enabled homes (or one home when a path filter is given): one attributed block per home — home_path, home_project, active marker, schema version note — with the projects read LIVE from that home's store at snapshot_at. A home that cannot be read localizes its error in its own block and keeps an honestly empty records list; other homes keep rendering. No copies are stored in the control plane (REQ-HOME-08).",
        },
        OpDescriptor {
            name: "home.tasks_all",
            class: OpClass::Read,
            input: "Option<home_path>",
            output: "Vec<HomeRecordsView<Task>>",
            description: "Merged tasks plane across enabled homes with the same per-home attribution, live resolution and localized-error semantics as home.projects_all; same-named tasks from different homes are distinct rows because blocks never collapse (per-home identity is the block, task ids stay inside it).",
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
    fn surface_version_v8_ships_the_name_plane_homes_profile_ratification_and_model_bundles() {
        // QA R5 (da/api-contract): the stamp and its increment are
        // pinned TOGETHER — @6 is by this test defined as "the @5
        // human-signboard + homes + ratification surface PLUS the
        // model-bundle plane" (task 01a1b71f): if the constant moves without
        // the new operations moving with it, or the model-bundle doors leave
        // the table while the stamp claims them, this fails. The literal is
        // the deliberate second writing of the version: bumping the
        // constant is an author's decision documented at the declaration
        // (see the bump mechanics comment there), and this test forces
        // that decision to be taken consciously, twice, in one change.
        // @15 (окно нового экрана): три ЧТЕНИЯ — правила оценки дома,
        // расход токенов по прогонам и кусок исходника по ссылке отчёта.
        // @16: ещё одно ЧТЕНИЕ — diff того же диапазона с полным контекстом
        // файла, чтобы окно разворачивало скрытые строки без второго чтения.
        // Ни одна версия не пишет: мутации @14 остаются как были.
        assert_eq!(SURFACE_VERSION, "api-surface@17");
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
        let homes = operations()
            .into_iter()
            .find(|o| o.name == "home.list")
            .expect("@4 surface must publish home.list (the homes registry plane)");
        assert_eq!(homes.class, OpClass::Read);
        // @5's own increment: the profile-rules ratification doors are
        // part of what the stamp claims.
        for name in ["profile.state", "profile.ratify", "profile.revoke"] {
            assert!(
                operations().iter().any(|o| o.name == name),
                "@5 surface must publish {name} (project-rules ratification plane)"
            );
        }
        // @6's own increment (task 01a1b71f): the model-bundle doors — one
        // read and the two writes — are part of what the stamp claims.
        let ops = operations();
        let snapshot = ops
            .iter()
            .find(|o| o.name == "model.snapshot")
            .expect("@6 surface must publish model.snapshot (REQ-MS-07)");
        assert_eq!(snapshot.class, OpClass::Read);
        for name in ["model.probe", "model.switch"] {
            let op = ops
                .iter()
                .find(|o| o.name == name)
                .unwrap_or_else(|| panic!("@6 surface must publish {name} (REQ-MS-03/04)"));
            assert_eq!(op.class, OpClass::Mutation);
        }
        // @7's own increment (task 01a1be5c): the bundle-authoring doors are
        // part of what the stamp claims.
        for name in [
            "model.bundle_check",
            "model.bundle_create",
            "model.bundle_update",
            "model.bundle_delete",
        ] {
            let op = ops
                .iter()
                .find(|o| o.name == name)
                .unwrap_or_else(|| panic!("@7 surface must publish {name} (REQ-MC-01..05)"));
            assert_eq!(op.class, OpClass::Mutation);
        }
        // @8's own increment: the form's handle read is part of what the
        // stamp claims (the window no longer derives it).
        let handle = ops
            .iter()
            .find(|o| o.name == "model.bundle_handle")
            .expect("@8 surface must publish model.bundle_handle (ASM-9)");
        assert_eq!(handle.class, OpClass::Read);
        // @11's own increment (task 01a1b60c): the derived agent status row is
        // a read, and the failure fact rides the two reads that already
        // existed (progress + the wait snapshot) — no separate mutation.
        let status = ops
            .iter()
            .find(|o| o.name == "agent.status")
            .expect("@11 surface must publish agent.status (REQ-AF-07)");
        assert_eq!(status.class, OpClass::Read);
        for name in ["task.progress", "inbox.snapshot"] {
            let op = ops
                .iter()
                .find(|o| o.name == name)
                .unwrap_or_else(|| panic!("@11: {name} stays published"));
            assert!(
                op.output.contains("agent_failure"),
                "@11: {name} must carry the agent-failure fact: {}",
                op.output
            );
        }
    }

    #[test]
    fn profile_ratification_operations_are_human_only_and_published() {
        // RV4: the act is marked HUMAN-ONLY in the published registry (the
        // enforcement is the actor rights of the human session, the registry
        // is where a second consumer SEES the constraint), and the state
        // view is a plain read.
        let ops = operations();
        for name in ["profile.ratify", "profile.revoke"] {
            let op = ops
                .iter()
                .find(|o| o.name == name)
                .unwrap_or_else(|| panic!("{name} must be published"));
            assert_eq!(op.class, OpClass::Mutation);
            assert!(
                op.description.starts_with("HUMAN-ONLY"),
                "{name}: a human-only act must say so in the registry"
            );
            // RV4 AC2: the registry names the SESSION gate too — a second
            // consumer reads here that a role run is refused by the API, not
            // told off by the transport.
            assert!(
                op.description.contains("human_only"),
                "{name}: the published constraint must name the typed refusal"
            );
        }
        let state = ops
            .iter()
            .find(|o| o.name == "profile.state")
            .expect("the rules state view is published");
        assert_eq!(state.class, OpClass::Read);
        // RV5: the ratification is a DISTINCT decision kind from the spec
        // approval — the vocabulary itself keeps them apart.
        assert_ne!(
            devai_core::types::decision_kind::PROFILE_RATIFICATION,
            devai_core::types::decision_kind::REQUIREMENTS_APPROVAL
        );
    }

    #[test]
    fn surface_version_v7_rides_the_slice_on_existing_reads() {
        // Task 01a1b5be (REQ-CV-14): the increment and its stamp move
        // together, but NO operation joins the table — the slice is carried
        // by the two reads that already existed.
        assert!(
            !operations().iter().any(|o| o.name == "task.convergence"),
            "REQ-CV-14: the convergence slice must not become its own operation"
        );
        let progress = operations()
            .into_iter()
            .find(|o| o.name == "task.progress")
            .expect("task.progress stays published");
        assert!(
            progress.output.contains("convergence"),
            "the progress shape carries the slice: {}",
            progress.output
        );
        let inbox = operations()
            .into_iter()
            .find(|o| o.name == "inbox.snapshot")
            .expect("inbox.snapshot stays published");
        assert!(
            inbox.output.contains("convergence"),
            "the inbox fact rides in the published shape: {}",
            inbox.output
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
            "profile.ratify",
            "profile.revoke",
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

    /// R1 quality return (da/api-contract): a descriptor must not declare
    /// `()` for an operation that hands a value back. `home.activate`
    /// declared `()` while the act returns the reopened `Api` handle — a
    /// second consumer building on the descriptor would have discarded it.
    /// The mechanical keeper: any operation whose own description promises
    /// a return cannot declare the empty output, and the one operation this
    /// cycle fixed is pinned by name.
    #[test]
    fn value_returning_operations_never_declare_the_empty_output() {
        let ops = operations();
        for o in &ops {
            let promises_a_value = o.description.contains("RETURN")
                || o.description.contains("Returns ")
                || o.description.contains("returns the")
                || o.description.contains("returns a");
            if promises_a_value {
                assert_ne!(
                    o.output, "()",
                    "{}: the description promises a value while output says ()",
                    o.name
                );
            }
        }
        let activate = ops
            .iter()
            .find(|o| o.name == "home.activate")
            .expect("home.activate is published");
        assert_eq!(
            activate.output, "Api",
            "home.activate returns the reopened observation handle, not ()"
        );
    }

    /// REQ-HOME-05/06: the homes plane is published, and its Registry class
    /// covers ONLY the home.* management operations — reading home records
    /// stays class Read, managing the registry stays class Registry, and no
    /// domain mutation ever hides in the registry plane.
    #[test]
    fn homes_plane_is_published_and_separated() {
        let ops = operations();
        let by_name: std::collections::HashMap<&str, &OpDescriptor> =
            ops.iter().map(|o| (o.name, o)).collect();
        for (name, class) in [
            ("home.list", OpClass::Read),
            ("home.projects_all", OpClass::Read),
            ("home.tasks_all", OpClass::Read),
            ("home.add", OpClass::Registry),
            ("home.set_enabled", OpClass::Registry),
            ("home.activate", OpClass::Registry),
        ] {
            let d = by_name
                .get(name)
                .unwrap_or_else(|| panic!("{name}: not published"));
            assert_eq!(d.class, class, "{name}: wrong class in the homes plane");
        }
        // Separation invariant: Registry exists only under `home.`.
        for o in &ops {
            if o.class == OpClass::Registry {
                assert!(
                    o.name.starts_with("home."),
                    "{}: a Registry-class op must live in the homes plane",
                    o.name
                );
            }
        }
    }
}
