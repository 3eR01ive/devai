// Типы фронтов devai-ui — зеркало serde-вывода devai-api (typed-views).
// Имена полей snake_case: структуры devai-core/devai-api не используют
// rename_all, enum-токены сериализуются SCREAMING_SNAKE_CASE. UI берёт
// готовые токены состояний как данные и никогда их не перефразирует.

export interface SessionInfo {
  home: string;
  actor: string;
  opened_at: string;
}

export interface OpDescriptor {
  name: string;
  class: "read" | "mutation";
  input: string;
  output: string;
  description: string;
}

export interface Project {
  id: string;
  name: string;
  description: string;
  repos: string[];
  created_at: string;
  updated_at: string;
}

export interface Task {
  id: string;
  project_id: string | null;
  title: string;
  business_description: string;
  state: string;
  return_state: string | null;
  requirements_seq: number;
  lock_version: number;
  created_at: string;
  updated_at: string;
}

export interface ContractRef {
  name: string;
  version: number;
}

export interface Artifact {
  id: string;
  task_id: string;
  kind: string;
  name: string;
  seq: number;
  category: string;
  status: string; // CURRENT | STALE | SUPERSEDED — хранимые данные
  sha256: string;
  size: number;
  media_type: string;
  contract: ContractRef | null;
  created_by: string;
  execution_id: string | null;
  depends_on: string[];
  created_at: string;
}

export interface Question {
  id: string;
  task_id: string;
  execution_id: string | null;
  text: string;
  context: string | null;
  options: string[];
  blocking: boolean;
  status: string; // OPEN | ANSWERED
  answer: string | null;
  answered_by: string | null;
  answered_at: string | null;
  created_at: string;
}

export interface StatusView {
  task: Task;
  allowed_operations: string[];
  current_artifacts: Artifact[];
  open_questions: Question[];
}

export interface PipelineView {
  order: string[];
  current: string;
  position: string | null;
  position_index: number | null;
  off_spine: boolean;
  terminal: boolean;
}

export interface RunningExecutionView {
  id: string;
  role: string;
  role_slug: string;
  role_version: number;
  agent_name: string | null;
  started_at: string;
  elapsed_seconds: number;
}

export interface ProgressView {
  task: Task;
  snapshot_at: string;
  pipeline: PipelineView;
  allowed_operations: string[];
  running_executions: RunningExecutionView[];
}

export interface ActiveRow {
  id: string;
  task_id: string;
  role: string;
  role_slug: string;
  role_version: number;
  agent_name: string | null;
  started_at: string;
  elapsed_seconds: number;
  state: string;
  stale: boolean;
  stale_terminal_task: boolean;
  stale_dead_owner: boolean;
  owner_pid: number | null;
  owner_host: string | null;
  owner_liveness: string; // alive | dead | unknown
  branch: string | null;
  worktree: string | null;
}

export interface ActiveSnapshot {
  snapshot_at: string;
  rows: ActiveRow[];
}

// Contract lexicon (rework R3/R4): these tokens are declared ONCE in
// `crates/devai-core/vocab.json`; the unions below are GENERATED from that
// data by `crates/devai-ui/build.rs` into `./vocab.generated.ts` and only
// re-exported here. Do not hand-write a token here — edit vocab.json and
// rebuild; `ts_types_match_the_rust_vocabulary` in `src/commands.rs` fails if
// the generated file stops matching the Rust enums.
import type { CheckStateToken, HumanItemKind } from "./vocab.generated";
export type { CheckStateToken, HumanItemKind } from "./vocab.generated";

export interface HumanFormItem {
  id: string;
  kind: HumanItemKind;
  summary: string;
  // The COMPLETE visible row, formatted once in Rust at assembly; the panel
  // prints it verbatim (rework R3: no second wording, no second precision).
  line: string;
  site: string;
  score: number | null;
  weight: number | null;
  importance: number | null;
  ratified_by?: string;
  reason?: string;
  evidence?: string[];
}

export interface HumanFormGroup {
  key: "blocks" | "nuances" | "anchors" | "unchecked";
  title: string;
  lead: string;
  items: HumanFormItem[];
}

export interface HumanForm {
  version: number;
  lang: string;
  headline: {
    verdict: string;
    machine_verdict: string;
    score: number;
    theta: number;
    // «S=…/1.00, порог …» — already formatted by the one Rust formatter.
    numbers: string;
    basis: string;
    counts: { blocks: number; nuances: number; anchors: number; unchecked: number };
  };
  groups: HumanFormGroup[];
  blocked_weight: number;
  max_blocked_weight: number;
}

export interface ReportFreshness {
  stale: boolean;
  marker: string;
  report_seq: number;
  report_status: string;
  task_state: string;
  execution: string | null;
  execution_status: string | null;
}

export interface QualityReportView {
  task: string;
  artifact: string;
  seq: number;
  contract: string;
  created_at: string;
  execution: string | null;
  status: string;
  report: unknown;
  report_text: string | null;
  // Что именно отрендерено: human_form | flat_text | corrupt_form |
  // corrupt_body | absent (rework R4) — UI различает форму и fallback, не
  // угадывая по тексту.
  source: string;
  freshness: ReportFreshness | null;
}

export interface TaskDependency {
  id: string;
  task_id: string;
  task_state: string;
  task_title: string;
  depends_on_id: string;
  depends_on_state: string;
  depends_on_title: string;
  kind: string; // parent | blocks
  actor: string;
  created_at: string;
}

export interface DependenciesView {
  task: Task;
  outgoing: TaskDependency[];
  incoming: TaskDependency[];
}

export interface Decision {
  id: string;
  task_id: string;
  kind: string;
  verdict: string;
  actor: string;
  rationale: string | null;
  refs: string[];
  created_at: string;
}

export interface WorkflowEvent {
  id: string;
  task_id: string | null;
  kind: string;
  actor: string;
  payload: unknown;
  created_at: string;
}

export interface RoleExecution {
  id: string;
  task_id: string;
  role_slug: string;
  role_version: number;
  agent_name: string | null;
  status: string; // RUNNING | COMPLETED | FAILED | CANCELLED
  inputs: string[];
  summary: string | null;
  error: string | null;
  started_at: string;
  finished_at: string | null;
  attempt?: number;
  parent_execution?: string | null;
  failure_class?: string | null;
  owner_pid?: number | null;
  owner_host?: string | null;
  owner_process_start?: number | null;
}

export interface LivenessView {
  id: string;
  task_id: string;
  status: string;
  owner_pid: number | null;
  owner_host: string | null;
  owner_process_start: number | null;
  liveness: string;
}

export interface UiError {
  code: string;
  message: string;
}

export type ArtifactContent =
  | { shape: "text"; artifact_id: string; text: string }
  | { shape: "binary"; artifact_id: string; size: number };

export interface InboxEntry {
  task: Task;
  open_questions: Question[];
}

// --- workspace view (task.workspace_changes / task.workspace_diff) ---
// state — verdict-токен платформы: no_isolation | live | historical |
// unavailable. UI показывает его как данные и никогда не перефразирует.
export interface WorkspaceFileChange {
  status: string; // A | M | D (git --no-renames)
  path: string;
}

export interface WorkspaceCommitStamp {
  execution: string;
  commit: string;
  at: string;
}

export interface WorkspaceChangesView {
  task: string;
  state: string;
  isolated: boolean;
  zero_changes: boolean;
  partial: boolean;
  branch: string | null;
  worktree: string | null;
  bound_head: string | null;
  tip: string | null;
  head_before: string | null;
  head_after: string | null;
  commits: WorkspaceCommitStamp[];
  files: WorkspaceFileChange[];
  note: string | null;
}

export interface WorkspaceDiffView {
  task: string;
  path: string;
  state: string;
  branch: string | null;
  baseline: string | null;
  tip: string | null;
  binary: boolean;
  size_bytes: number | null;
  diff: string | null;
  note: string | null;
}
