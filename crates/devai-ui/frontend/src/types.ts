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
  // REQ-TN-01: human signboard of the task (analyst's name, stamped at
  // spec publication; editable by the human `task rename`). null = имени
  // ещё нет. Формальный title остаётся текстом создавшего всегда.
  human_name: string | null;
  // REQ-TN-03: the FINISHED signboard, computed ONCE by the producer
  // (`devai_core::types::Task::display_sign`, serialized as `display`):
  // human name → first sentence of the DRAFT statement → formal title.
  // The frontend renders this string verbatim and NEVER re-derives it
  // (no-read-time-rederivation): there is no client-side copy of the rule.
  display: string;
  state: string;
  return_state: string | null;
  requirements_seq: number;
  lock_version: number;
  created_at: string;
  updated_at: string;
}

// REQ-TN-03 / R1: the signboard comes from the API as finished data
// (`Task.display`) — the ONE rule lives in devai-core and is computed on
// the producer side. This file holds NO client-side copy of the rule:
// no `firstSentence`, no re-derivation per renderer (no-read-time-
// rederivation). Views print `task.display` / `edge.task_display`.

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
    counts: {
      blocks: number;
      nuances: number;
      anchors: number;
      unchecked: number;
      // fully passed points (1.0, no decision link): the norm, counted not
      // listed — absent in v1 forms, so optional.
      passed?: number;
    };
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
  // Ratified OQ-TN-5: готовые вывески обоих концов ребра (правило REQ-TN-03
  // посчитано read-плоскостью); title остаётся машиночитаемым следом.
  task_display: string;
  depends_on_id: string;
  depends_on_state: string;
  depends_on_title: string;
  depends_on_display: string;
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

export interface WaitRow {
  task: Task;
  /** Что платформа требует ОТ оператора (REQ-GATE-03 AC1), не свойство задачи. */
  demand: string;
  cli_hint: string;
  open_questions: Question[];
  spec: SpecPresentation | null;
  report: QualityReportView | null;
  report_note: string | null;
  /**
   * Факт сходимости гейта (REQ-CV-09/10): стадия ЭТОЙ строки —
   * spec_review для APPROVAL, quality для QA. Вне двух гейтовых состояний
   * null: график и плейсхолдер не рисуются. Значения (балл, режимы,
   * причины) уже посчитаны платформой и здесь только раскладываются.
   */
  convergence: ConvergenceFact | null;
}

// --- сходимость стадии (task.convergence, REQ-CV-05/09) ---
// mode_note и missing — машинные токены платформы; человеческие фразы для
// них — фиксированный словарь UI (convergence_view.ts), не свободный текст.
export interface ConvergencePoint {
  round: number;
  artifact_id: string;
  seq: number;
  status: string;
  created_at: string;
  measured: boolean;
  missing: string[];
  profile: string | null;
  comparable: boolean;
  time_seconds: number | null;
  score: number | null;
  score_milli: number | null;
  theta_milli: number | null;
  low_count: number | null;
  unscored_count: number | null;
  closed_count: number | null;
  delta_score_milli: number | null;
  delta_low: number | null;
  distance: number | null;
  requirements_count: number | null;
  oq_count: number | null;
  verdict: string | null;
  delta_requirements: number | null;
  delta_oq: number | null;
  modes: string[];
  mode_note: string | null;
  stuck_applicable: boolean;
}

export interface ConvergenceCounters {
  unmeasured: number;
  spec_versions_without_review: number;
  rounds_without_time: number;
}

export interface StageConvergence {
  stage: string;
  series: ConvergencePoint[];
  counters: ConvergenceCounters;
  stage_total_seconds: number;
}

export interface ConvergenceView {
  task: string;
  snapshot_at: string;
  gap_milli: number;
  reason: string | null;
  stages: StageConvergence[];
}

/** Один круг мини-графика строки «Ожидания» (REQ-CV-09). */
export interface ConvergenceTick {
  round: number;
  measured: boolean;
  score_milli: number | null;
  theta_milli: number | null;
  requirements_count: number | null;
  oq_count: number | null;
  modes: string[];
}

/** Факт гейтовой строки: серия без тел отчётов, режимы последнего круга. */
export interface ConvergenceFact {
  stage: string;
  gap_milli: number;
  rounds: number;
  series: ConvergenceTick[];
  modes: string[];
  mode_note: string | null;
  missing: string[];
}

// Предъявление APPROVAL-строки (REQ-GATE-04/05): тело REQUIREMENTS_SPEC,
// пришедшее в снимке секции (`inbox_view` → `inbox.snapshot`, НЕ отдельный
// artifact.cat_text) и разобранное на стороне предъявления
// без изменения данных. paragraph_source — честная метка происхождения
// абзаца (ратифицированное OQ-GATE-4), open_questions — вопросы ДОКУМЕНТА,
// не platform-вопросы реестра (у них нет id и их не отвечают question.answer).
export interface SpecPresentation {
  artifact: Artifact;
  current: boolean; // false = не CURRENT, approve невозможен (OQ-GATE-3)
  human_paragraph: string | null;
  paragraph_source: "spec" | "business_description";
  open_questions: string[];
  unavailable_note: string | null;
}

// Справочная группа ниже списка стоящих (ратифицированное OQ-GATE-1):
// безответный НЕблокирующий вопрос — заметка на активной задаче, платформа
// на ней не стоит (workflow.rs:747-748), поэтому это не wait-row. Но строкой
// снапшота она является, и бейдж её считает: badge == waiting + human_stopped
// + notes (REQ-GATE-08 AC3, commands.rs::compose_wait_snapshot).
export interface WaitNote {
  task: Task;
  question: Question;
}

export interface WaitSnapshot {
  waiting: WaitRow[];
  human_stopped: WaitRow[];
  notes: WaitNote[];
  badge: number;
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
