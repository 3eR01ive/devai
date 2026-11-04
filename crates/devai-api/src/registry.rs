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

/// The full registry of operations the API surface publishes. Cycle 1 ships
/// the read plane as working code; every other entry names the operation
/// whose implementation lands with the mutation plane (cycle 2).
pub fn operations() -> Vec<OpDescriptor> {
    // NOTE: kept as one flat const-style table — the registry IS data here,
    // and `devai api registry --json` renders it verbatim.
    vec![
        // ---- project ----
        OpDescriptor { name: "project.add", class: OpClass::Mutation, input: "ProjectAddReq{name, description, repos}", output: "Project",
            description: "Register a project with optional local repository paths; inserts one projects row. No workflow state involved." },
        OpDescriptor { name: "project.list", class: OpClass::Read, input: "()", output: "Vec<Project>",
            description: "List all registered projects ordered by name." },
        OpDescriptor { name: "project.get", class: OpClass::Read, input: "id_or_name", output: "Option<Project>",
            description: "Look up one project by full id or name; None when absent." },
        // ---- task: creation & reads ----
        OpDescriptor { name: "task.create", class: OpClass::Mutation, input: "TaskCreateReq{title, project_id, description, parent, blocked_by}", output: "TaskCreated{task, linked}",
            description: "Create a task in DRAFT and register 'parent'/'blocks' dependency edges at the moment of creation (REQ-DEP-07); dangling references fail loudly. The project name resolves through the `project.get` read, so its not-found refusal keeps the exact position the command had." },
        OpDescriptor { name: "task.list", class: OpClass::Read, input: "()", output: "Vec<Task>",
            description: "List all tasks ordered by most recently updated." },
        OpDescriptor { name: "task.get", class: OpClass::Read, input: "id_prefix", output: "Task",
            description: "Resolve one task by full id or unique id prefix; fails with not_found/ambiguous. Mirrors `task get`." },
        OpDescriptor { name: "task.status", class: OpClass::Read, input: "id_prefix", output: "StatusView{task, allowed_operations, current_artifacts, open_questions}",
            description: "Current state plus the operations the platform permits next, the CURRENT artifacts and open questions. Mirrors `task status`." },
        OpDescriptor { name: "task.progress", class: OpClass::Read, input: "id_prefix", output: "ProgressView{task, snapshot_at, pipeline, allowed_operations, running_executions}",
            description: "Pipeline position, re-entry note and the snapshot of RUNNING executions, drawn purely from engine data. Mirrors `task progress`." },
        OpDescriptor { name: "task.active", class: OpClass::Read, input: "()", output: "ActiveSnapshot{snapshot_at, rows}",
            description: "Cross-task read-only snapshot of every RUNNING execution with liveness triage (alive/dead/unknown) and worktree link only where the platform recorded worktree.prepared. Mirrors `task active`." },
        OpDescriptor { name: "task.report", class: OpClass::Read, input: "id_prefix", output: "QualityReportView",
            description: "The CURRENT quality report the human signs at the QA gate: artifact identity, contract, machine body and rendering. Mirrors `task report`." },
        OpDescriptor { name: "task.events", class: OpClass::Read, input: "id_prefix, limit", output: "Vec<WorkflowEvent>",
            description: "Immutable event history of the task, newest tail limited. Mirrors `task events`." },
        OpDescriptor { name: "task.decisions", class: OpClass::Read, input: "id_prefix", output: "Vec<Decision>",
            description: "Decision rows recorded for the task (approvals, overrides, ratifications). Backs APPROVED_DECISIONS delivery to roles; the CLI exposes it as `task decisions`." },
        OpDescriptor { name: "task.dependencies", class: OpClass::Read, input: "id_prefix", output: "DependenciesView{task, outgoing, incoming, epic_children, own_parents}",
            description: "Typed dependency view of one task (outgoing/incoming ACTIVE edges); the epic vertex role reads out of the same data. Mirrors `task dependencies`." },
        OpDescriptor { name: "task.epics", class: OpClass::Read, input: "()", output: "Vec<EpicVertex>",
            description: "Epic registry as a VIEW over the dependency graph (no epic entity exists). Mirrors `task epics`." },
        OpDescriptor { name: "task.graph", class: OpClass::Read, input: "()", output: "GraphView{nodes, ready}",
            description: "The task dependency graph with states and the ready set (no open blockers, non-terminal), built purely from stored data. Mirrors `task graph`." },
        // ---- task: workflow mutations (engine commands; working code in
        // crates/devai-api/src/mutations.rs — the dispatch `task_op` and
        // every named operation are the same path, REQ-MUT-05 AC3) ----
        OpDescriptor { name: "task.start", class: OpClass::Mutation, input: "id_prefix", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "DRAFT → ANALYSIS through workflow::start. Mirrors `task start`." },
        OpDescriptor { name: "task.run", class: OpClass::Mutation, input: "RunReq{id, agent, workspace, force_reaudit}", output: "RunOutcome{report, current_quality_report, gate_report_text, allowed_operations}",
            description: "Drive assigned roles through all autonomous stages to the next human gate; refuses while a RUNNING execution exists (single writer). force_reaudit carries the --force-reaudit gate semantics. Mirrors `task run`." },
        OpDescriptor { name: "task.revise", class: OpClass::Mutation, input: "ReviseReq{id, name, content, reason}", output: "ReviseOutcome{task, staled_ids, allowed_operations}",
            description: "Publish/replace the REQUIREMENTS_SPEC and move to REVIEW through the contract gate. Mirrors `task revise`." },
        OpDescriptor { name: "task.review", class: OpClass::Mutation, input: "id_prefix, verdict, summary", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Record the reviewer verdict in REVIEW. Mirrors `task review`." },
        OpDescriptor { name: "task.approve", class: OpClass::Mutation, input: "id_prefix, rationale", output: "ApproveOutcome{mutation: MutationResult, decision: Decision}",
            description: "APPROVAL → IMPLEMENTATION: requires a CURRENT REQUIREMENTS_SPEC; persists the workflow event AND a requirements_approval decision row. Mirrors `task approve`." },
        OpDescriptor { name: "task.reject", class: OpClass::Mutation, input: "id_prefix, rationale", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "APPROVAL → ANALYSIS. Mirrors `task reject`." },
        OpDescriptor { name: "task.accept", class: OpClass::Mutation, input: "id_prefix, rationale", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "QA → ACCEPTED; refuses while open 'blocks' dependencies stand unless a dependency_override decision covers every blocker. Mirrors `task accept`." },
        OpDescriptor { name: "task.return", class: OpClass::Mutation, input: "id_prefix, rationale", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "QA → REWORK. Mirrors `task return`." },
        OpDescriptor { name: "task.pause", class: OpClass::Mutation, input: "id_prefix, reason", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Park the task in PAUSED. Mirrors `task pause`." },
        OpDescriptor { name: "task.resume", class: OpClass::Mutation, input: "id_prefix", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Resume a PAUSED/BLOCKED/FAILED task or answer-driven restore, subject to the dependency gate. Mirrors `task resume`." },
        OpDescriptor { name: "task.block", class: OpClass::Mutation, input: "id_prefix, reason", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Move the task to BLOCKED. Mirrors `task block`." },
        OpDescriptor { name: "task.cancel", class: OpClass::Mutation, input: "id_prefix, reason", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Terminal CANCELLED. Mirrors `task cancel`." },
        OpDescriptor { name: "task.fail", class: OpClass::Mutation, input: "id_prefix, reason", output: "MutationResult{task, previous_state, event_kinds, staled, superseded, allowed_operations}",
            description: "Record a runtime failure; the task parks in FAILED for `resume`. Mirrors `task fail`." },
        OpDescriptor { name: "task.depend", class: OpClass::Mutation, input: "DependReq{id, on, kind}", output: "TaskDependency",
            description: "Register a typed ACTIVE dependency edge, loud at write on self-reference, duplicate, dangling or intra-kind cycles. Mirrors `task depend`." },
        OpDescriptor { name: "task.unlink", class: OpClass::Mutation, input: "DependReq{id, on, kind}", output: "UnlinkResult{edge, superseded_by}",
            description: "Supersede an ACTIVE edge; the row stays as history immutable (OQ7) and the edge is returned AS IT WAS while ACTIVE — the row's own actor is the original author, the superseder is `superseded_by`. Mirrors `task unlink`." },
        // ---- human-only gate escapes (RATIFIED OQ-13: v1 privilege of the
        // human operator — published so a consumer sees them, enforced by the
        // actor rights of the calling session, never by the transport) ----
        OpDescriptor { name: "task.override", class: OpClass::Mutation, input: "id_prefix, blockers, rationale", output: "OverrideResult{task, override_blockers}",
            description: "HUMAN-ONLY (OQ-13): record a dependency_override ratification to continue despite open blockers; refs bound to exactly the covered ids. Mirrors `task override`." },
        OpDescriptor { name: "task.ratify_empty_rework", class: OpClass::Mutation, input: "id_prefix, rationale", output: "RatifyResult{task, decision, recorded_now, prev_report, current_report, state, allowed_operations}",
            description: "HUMAN-ONLY (OQ-13): the EMPTY-REWORK GATE escape — records rework_noop_ratified for the exact report pair, releases the gate question, runs the withheld REWORK → QA transition. Mirrors `task ratify-empty-rework`." },
        OpDescriptor { name: "task.merge", class: OpClass::Mutation, input: "MergeRecordReq{id, branch, workspace, head_before, head_after, rationale}", output: "MergeResult{task_id, branch, workspace, head_before, head_after}",
            description: "HUMAN-ONLY (OQ-13): the persistence half of the single merge path — records the `rework_merge` decision and `rework.merged` event for a merge of the platform rework/<task-id> branch into the checked-out baseline. The repository act itself stays in the human-invoked command (REQ-WT-07 keeps the git invocation behind the CLI subcommand so no engine path reaches it); this operation never touches task state. Mirrors the recording inside `task merge`." },
        // ---- artifacts ----
        OpDescriptor { name: "artifact.publish", class: OpClass::Mutation, input: "PublishReq{id, kind, name, content, depends, contract, execution, verdict, category}", output: "PublishOutcome{publish: PublishedRow{artifact, task}, advanced: Option<AdvanceOutcome>}",
            description: "Publish a versioned artifact through the contract gate; certain kinds advance the workflow through the SAME core commands. Requires a RUNNING execution row when execution is given. Mirrors `artifact publish`." },
        OpDescriptor { name: "artifact.publish_row", class: OpClass::Mutation, input: "PublishReq{id, kind, name, content, depends, contract, execution, verdict, category}", output: "PublishedRow{artifact, task}",
            description: "The first phase of `artifact.publish` alone: the row through the contract gate, no workflow advance. Exists beside the compound so a consumer that renders between the phases (the CLI, printing the artifact row before the advance) keeps output parity even when the advance fails." },
        OpDescriptor { name: "artifact.advance", class: OpClass::Mutation, input: "AdvanceReq{id, artifact_id, kind, verdict}", output: "AdvanceOutcome{from, to, rework_gate}",
            description: "The second phase of `artifact.publish`: the workflow advance certain kinds carry (requirements → REVIEW, review report → gate, implementation report → QA through the empty-rework gate). Same core commands as the compound and the engine." },
        OpDescriptor { name: "artifact.list", class: OpClass::Read, input: "id_prefix", output: "Vec<Artifact>",
            description: "All artifact rows of the task with versions and statuses. Mirrors `artifact list`." },
        OpDescriptor { name: "artifact.cat", class: OpClass::Read, input: "artifact_id", output: "Vec<u8> (raw bytes)",
            description: "Fetch the stored content bytes of one artifact. Mirrors `artifact cat`." },
        OpDescriptor { name: "artifact.cat_text", class: OpClass::Read, input: "artifact_id", output: "String",
            description: "Text presentation of artifact content; NotUtf8 error when the bytes are not text." },
        // ---- questions ----
        OpDescriptor { name: "question.ask", class: OpClass::Mutation, input: "AskReq{id, text, context, options, blocking, execution}", output: "QuestionAsked{question, parked}",
            description: "Raise a question; a blocking one parks the task in WAITING_FOR_INPUT through workflow::raise_question. Mirrors `question ask`." },
        OpDescriptor { name: "question.answer", class: OpClass::Mutation, input: "question_id, text", output: "AnsweredOutcome{question_id, from_state, to_state}",
            description: "Answer an OPEN question and run the answer-driven transition. Mirrors `question answer`." },
        OpDescriptor { name: "question.list", class: OpClass::Read, input: "id_prefix", output: "Vec<Question>",
            description: "All questions of the task. Mirrors `question list`." },
        // ---- roles / agents / contracts ----
        OpDescriptor { name: "role.add", class: OpClass::Mutation, input: "file (RoleDefinition JSON)", output: "RoleDefinition",
            description: "Register a versioned RoleDefinition; version 0 in the file means latest+1 for the slug. Mirrors `role add`." },
        OpDescriptor { name: "role.list", class: OpClass::Read, input: "()", output: "Vec<RoleDefinition>",
            description: "List all registered role versions." },
        OpDescriptor { name: "role.get", class: OpClass::Read, input: "slug", output: "Option<RoleDefinition>",
            description: "Latest definition for a role slug." },
        OpDescriptor { name: "agent.add", class: OpClass::Mutation, input: "name, provider, model, runtime", output: "AgentInstance",
            description: "Register a local agent instance; credentials stay outside the store. Mirrors `agent add`." },
        OpDescriptor { name: "agent.list", class: OpClass::Read, input: "()", output: "Vec<AgentInstance>",
            description: "List registered agents." },
        OpDescriptor { name: "contract.add", class: OpClass::Mutation, input: "ContractAddReq{name, version, schema}", output: "ContractRegistration{name, version}",
            description: "Register a versioned contract schema. Mirrors `contract add`." },
        OpDescriptor { name: "contract.get", class: OpClass::Read, input: "name, version", output: "Option<String> (schema JSON text)",
            description: "Fetch a stored contract schema. Mirrors `contract get`." },
        // ---- executions ----
        OpDescriptor { name: "exec.start", class: OpClass::Mutation, input: "ExecStartReq{id, role, agent, inputs}", output: "ExecStarted{execution, task}",
            description: "Start a role execution: role/stage matrix validated, workspace revision and owner columns captured platform-side, single-writer guard enforced. Mirrors `exec start`." },
        OpDescriptor { name: "exec.finish", class: OpClass::Mutation, input: "exec_id, summary", output: "RoleExecution",
            description: "Close a RUNNING row as COMPLETED. Mirrors `exec finish`." },
        OpDescriptor { name: "exec.fail", class: OpClass::Mutation, input: "exec_id, error", output: "RoleExecution",
            description: "Close a RUNNING row as FAILED with the error text. Mirrors `exec fail`." },
        OpDescriptor { name: "exec.cancel", class: OpClass::Mutation, input: "exec_id", output: "RoleExecution",
            description: "Close a RUNNING row as CANCELLED (cooperative; the process is never touched, REQ-SW-06). Mirrors `exec cancel`." },
        OpDescriptor { name: "exec.list", class: OpClass::Read, input: "Option<task_ref>", output: "Vec<RoleExecution>",
            description: "Execution rows, optionally for one task; attempt/failure_class lineage ships in the rows. Mirrors `exec list`." },
        OpDescriptor { name: "exec.get", class: OpClass::Read, input: "exec_id_prefix", output: "RoleExecution",
            description: "One execution row by full id or unique prefix — machine observation of a running process's state without the launching terminal (REQ-API-07). Backs `exec get`." },
        OpDescriptor { name: "exec.liveness", class: OpClass::Read, input: "exec_id_prefix", output: "LivenessView{id, status, owner_pid, owner_host, liveness}",
            description: "Liveness verdict (alive/dead/unknown) for the recorded owner of one execution — orphans distinguishable by data, not pgrep. Backs `exec liveness`." },
        OpDescriptor { name: "exec.active_rows", class: OpClass::Read, input: "()", output: "Vec<ActiveRow>",
            description: "RUNNING rows with per-row liveness, ordered oldest-first — the data behind `task active`; the task plane wraps it into ActiveSnapshot." },
        // ---- export / import ----
        OpDescriptor { name: "export.bundle", class: OpClass::Read, input: "id_prefix, out_dir", output: "BundleManifest",
            description: "Export the task bundle (domain data + artifacts, no caches/credentials) — identity and versions identical to the CLI path (REQ-API-02 AC3). Mirrors `export`." },
        OpDescriptor { name: "import.bundle", class: OpClass::Mutation, input: "dir", output: "ImportOutcome",
            description: "Import a bundle, respecting single-CURRENT and single-writer invariants. Mirrors `import`." },
        // ---- registry ----
        OpDescriptor { name: "api.registry", class: OpClass::Read, input: "()", output: "Vec<OpDescriptor>",
            description: "The published operation registry itself — every operation with typed input/output names and textual semantics. Backs `devai api registry`." },
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
        assert_eq!(seen.len(), total, "duplicate operation names in the registry");
        for o in operations() {
            assert!(o.name.contains('.'), "{}: name must be area.action", o.name);
        }
    }

    #[test]
    fn every_operation_carries_textual_semantics() {
        // REQ-API-01 AC2 / REQ-API-06 AC1: a description is stored data,
        // never a bare code reference.
        for o in operations() {
            assert!(o.description.trim().len() >= 20, "{}: description too thin", o.name);
            assert!(!o.input.is_empty() && !o.output.is_empty(), "{}: shapes must be named", o.name);
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
    fn init_seed_exception_is_not_an_operation() {
        // RATIFIED OQ-10 (encapsulated per cycle-2 Q2): the bootstrap pair
        // is not an operation of the registry in either class — the API
        // owns the handle (`Api::open`/`seed_config`), consumers never do.
        for o in operations() {
            assert!(!o.name.starts_with("init") && !o.name.starts_with("seed"), "{}: OQ-10 exception leaked into the registry", o.name);
        }
    }

    #[test]
    fn mutation_plane_is_fully_registered() {
        // REQ-MUT-05: every mutation the cycle-2 surface implements has a
        // registry entry of class Mutation. The list mirrors the public
        // operations in crates/devai-api/src/mutations.rs.
        let muts: std::collections::HashSet<&str> = operations().iter().filter(|o| o.class == OpClass::Mutation).map(|o| o.name).collect();
        for n in [
            "project.add", "task.create", "task.start", "task.run", "task.revise", "task.review", "task.approve", "task.reject", "task.accept", "task.return",
            "task.pause", "task.resume", "task.block", "task.cancel", "task.fail", "task.depend", "task.unlink", "task.override", "task.ratify_empty_rework", "task.merge",
            "artifact.publish", "artifact.publish_row", "artifact.advance", "question.ask", "question.answer", "role.add", "agent.add", "contract.add", "exec.start",
            "exec.finish", "exec.fail", "exec.cancel", "import.bundle",
        ] {
            assert!(muts.contains(n), "{n}: mutation of the API surface missing from the registry");
        }
    }
}
