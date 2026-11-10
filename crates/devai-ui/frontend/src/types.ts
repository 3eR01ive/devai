// Типы фронтов devai-ui — зеркало serde-вывода devai-api (typed-views).
// Имена полей snake_case: структуры devai-core/devai-api не используют
// rename_all, enum-токены сериализуются SCREAMING_SNAKE_CASE. UI берёт
// готовые токены состояний как данные и никогда их не перефразирует.
import type { LanState } from "./lanState";

export type { LanState };

export interface SessionInfo {
  home: string;
  actor: string;
  opened_at: string;
  // homes-registry cycle: где живёт реестр (OQ-HOME-1) и честно
  // простампованный факт деградированного открытия активного дома.
  control_home: string;
  degraded_from: HomeFailure | null;
}

// Типизированная причина, по которой дом не открылся на bootstrap-границе
// (R5): code — машинный токен домена (home_unavailable / home_no_store /
// home_schema_version), home — путь, message — операторский текст.
export interface HomeFailure {
  code: string;
  home: string;
  message: string;
}

export interface OpDescriptor {
  name: string;
  class: "read" | "mutation" | "registry";
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
  /** REQ-QG-08/09: per-action availability at the QA gate; null outside QA. */
  quality_gate: QualityGateView | null;
}

/** One platform refusal of a QA-gate act: a stable code plus the sentence the
 * window prints verbatim (the UI never derives availability from task state). */
export interface QualityGateDenial {
  code: string;
  text: string;
}

/** The QA gate's public availability (task_status, REQ-QG-08/09). */
export interface QualityGateView {
  audit_running: boolean;
  current_report: string | null;
  open_blockers: string[];
  may_accept: boolean;
  accept_denied: QualityGateDenial | null;
  may_return: boolean;
  return_denied: QualityGateDenial | null;
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
  /** Срез сходимости (REQ-CV-12); null только у терминальных задач. */
  convergence: ConvergenceView | null;
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
  /** REQ-QG-09 AC3: «идёт аудит» for a live quality audit, null otherwise. */
  note: string | null;
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

// The FIRST LAYER of a stored report (REQ-SAY-01..06): read off the stored
// human form by the platform, printed verbatim here. It carries the human
// wording («что нашёл аудит», чем грозит каждое замечание, что делать с
// непроверенным) and NEVER a figure; every number stays in the stored row.
export interface FirstLayerRow {
  id: string;
  what: string;
  // Empty when the report named no site; the reader says «место не указано».
  site: string;
  // A finding: what it costs. An unchecked point: what to do about it.
  note: string;
  // Unchecked points only: what exactly was missing (the stored reason).
  missing?: string;
  // The stored machine row (id · score · weight · importance · site).
  line: string;
  evidence?: string[];
}

export interface FirstLayer {
  found: string;
  findings: FirstLayerRow[];
  unchecked: FirstLayerRow[];
  unchecked_lead: string;
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
  // REQ-SAY-01..06: человеческий первый слой того же отчёта; null — у тела
  // нет читаемой человеческой формы (старый плоский текст / повреждение).
  first_layer: FirstLayer | null;
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
  // The v11 run stamp (REQ-MS-05): «what did this run actually use?».
  // Absent/null = «связка не записана» (a row written before the rung).
  bundle?: ExecutionBundle | null;
}

export interface ExecutionBundle {
  driver: string;
  llm_provider: string;
  model: string;
  reasoning: string;
}

// --- model-bundle section (model.snapshot / model.probe / model.switch) ---
// REQ-MS-01/06/07: three INDEPENDENT identity fields (driver, llm_provider,
// model), ONE status field, the declared allow-list and the stored history.
export interface ModelBundleView {
  id: string;
  label: string;
  agent: string;
  driver: string;
  llm_provider: string;
  /** Operator word of `llm_provider`, served by the platform dictionary. */
  llm_provider_text: string;
  model: string;
  /** Operator word of `model` («не передаётся» when empty). */
  model_text: string;
  /** The level of thinking — part of the bundle (the ratified OQ-6 answer). */
  reasoning: string;
  /** Operator word of `reasoning` («связка неполна» when empty). */
  reasoning_text: string;
  probe_phrase: string;
  probe_timeout_ms: number;
  fresh_secs: number;
  active: boolean;
}

export interface ActiveBundleView {
  id: string | null;
  label: string | null;
  agent: string;
  driver: string;
  llm_provider: string;
  /** Operator word of `llm_provider`, served by the platform dictionary. */
  llm_provider_text: string;
  model: string;
  /** Operator word of `model` — served, never re-derived in the window. */
  model_text: string;
  /** Empty = an incomplete bundle (a run does not start). */
  reasoning: string;
  /** Operator word of `reasoning` — served, never re-derived. */
  reasoning_text: string;
  declared: boolean;
}

export interface BundleStatusView {
  // live | not_working | not_proven | unknown | not_selected
  state: string;
  /** The ONE operator word of `state`, served by the platform dictionary. */
  state_text: string;
  latency_ms: number | null;
  verdict: string | null;
  reason: string | null;
  /** The operator word of `reason`, from the same platform dictionary. */
  reason_text: string | null;
  evidence: string | null;
  evidence_at: string | null;
  freshness: string;
  /** The operator word of `freshness` (names WHICH evidence is fresh). */
  freshness_text: string;
  probe_older: boolean;
}

export interface ModelEventView {
  at: string;
  kind: string;
  actor: string;
  bundle_id: string | null;
  verdict: string | null;
  outcome: string | null;
  /** The operator word of `outcome` for a bundle mutation (served). */
  outcome_text?: string | null;
  reason: string | null;
  /** The operator word of `reason`, from the platform dictionary. */
  reason_text: string | null;
  latency_ms: number | null;
  from: Record<string, unknown> | null;
  to: Record<string, unknown> | null;
  agent_changed: boolean;
}

export interface RunningBundleRow {
  execution_id: string;
  task_id: string;
  agent: string | null;
  started_at: string;
}

export interface FailuresBanner {
  code: string;
  text: string;
  suggestion: string | null;
}

export interface ModelSnapshot {
  snapshot_at: string;
  active: ActiveBundleView | null;
  status: BundleStatusView;
  declared: ModelBundleView[];
  history: ModelEventView[];
  failures: FailuresBanner | null;
  running: RunningBundleRow[];
  /** The declared file edition the form must carry back on save. */
  file_revision: string;
  /** The closed vocabulary the form may offer (drivers, factory probe). */
  form: BundleFormView;
}

export interface BundleFormView {
  drivers: string[];
  self_provider_driver: string;
  default_phrase: string;
  default_timeout_ms: number;
  default_fresh_secs: number;
}

/** The probe policy — the SAME nested shape the declaration carries, so the
 * form and the file share one field catalogue (no flat `probe_*` copies). */
export interface ProbePolicyInput {
  phrase: string;
  timeout_ms: number;
  fresh_secs: number;
}

/** The handle a NEW bundle will carry, as the SERVER derives it (ASM-9).
 * The window prints `id` verbatim and never derives it itself. */
export interface BundleHandleView {
  id: string;
}

/** The form payload — the declaration's own field names, nothing else. */
export interface BundleFormInput {
  id: string;
  label: string;
  agent: string;
  driver: string;
  llm_provider: string;
  model: string;
  reasoning: string;
  probe: ProbePolicyInput;
  editing: boolean;
  file_revision: string;
}

export interface BundleMutationView {
  outcome: string; // created | updated | deleted
  outcome_text: string;
  bundle_id: string;
  label: string;
  verdict: string | null;
  latency_ms: number | null;
  agent_changed: boolean;
  revision: string;
}

export interface ProbeView {
  bundle_id: string;
  verdict: string;
  /** The operator word of `verdict`, from the platform dictionary. */
  verdict_text: string;
  latency_ms: number;
  exit_code: number | null;
  output_excerpt: string;
  at: string;
}

export interface SwitchResultView {
  outcome: string; // applied | refused_probe
  bundle_id: string;
  verdict: string;
  latency_ms: number;
  code: string;
  /** The switch changed the agent NAME: in-flight runs stay on the old one. */
  agent_changed: boolean;
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
   * Срез сходимости ЗАДАЧИ (REQ-CV-12): тот же объект, что отдаёт чтение
   * хода задачи; null у терминальных задач и у строк вне секции ожидания.
   * Строка выбирает стадию для графика сама (гейт → его стадия, иное —
   * последняя).
   */
  convergence: ConvergenceView | null;
}

// --- сходимость стадии (REQ-CV-05/09/12) ---
// mode, rule, reason, token, volume_direction — машинные токены платформы;
// человеческие фразы для них — фиксированный словарь UI
// (convergence_view.ts), не свободный текст.
export interface MissingEntry {
  field: string;
  token: string;
}

export interface ModeBasis {
  rule: string;
  reason: string | null;
  rounds: number[];
  check_id: string | null;
}

/** Пробел данных — объяснение токена платформы (закрытый словарь). */
export interface ReadError {
  code: string;
  message: string;
}

/** Одна точка — одна форма для обеих стадий, все ключи всегда на месте. */
export interface ConvergencePoint {
  round: number;
  artifact_id: string;
  created_at: string;
  stage: string;
  status: string;
  measured: boolean;
  comparable: boolean;
  missing: MissingEntry[];
  score_milli: number | null;
  theta_milli: number | null;
  low_count: number | null;
  unscored_count: number | null;
  invalid_count: number | null;
  pending_count: number | null;
  closed_count: number | null;
  delta_score_milli: number | null;
  verdict: string | null;
  requirements_count: number | null;
  oq_count: number | null;
  delta_requirements: number | null;
  delta_oq: number | null;
  volume_direction: string | null;
  time_seconds: number | null;
  profile: string | null;
  mode: string;
  mode_basis: ModeBasis;
  /** Сбой чтения из стора (REQ-CV-05 + QA R2); null — фолта не было. */
  read_error: ReadError | null;
}

export interface ConvergenceCounters {
  rounds_total: number;
  rounds_unmeasured: number;
  rounds_without_time: number;
  spec_versions_without_review: number;
}

export interface StageConvergence {
  stage: string;
  series: ConvergencePoint[];
  counters: ConvergenceCounters;
  stage_total_seconds: number | null;
  stage_current_seconds: number | null;
  missing: MissingEntry[];
}

export interface ConvergenceView {
  gap_milli: number;
  stages: StageConvergence[];
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

// --- signature card (task_signature, REQ-SIGN-07) ---------------------
// Один ответ на всю секцию подписи: абзац, требования, вопросы документа,
// блок ревьювера, дифф к прошлой версии, оба права подписи и самый новый
// сохранённый факт подписи. UI ничего не парсит и не досочиняет.
export interface RequirementView {
  id: string;
  statement: string;
  acceptance_criteria: string[];
  constraints: string[];
}

export interface SpecView {
  artifact: Artifact;
  current: boolean;
  human_paragraph: string | null;
  paragraph_source: "spec" | "business_description";
  requirements: RequirementView[];
  requirements_note: string | null;
  open_questions: string[];
  unavailable_note: string | null;
}

export interface ReviewView {
  artifact: Artifact;
  current: boolean;
  verdict: string | null;
  human_paragraph: string | null;
  paragraph_source: "review_report" | "summary" | "none";
  note: string | null;
}

export interface DiffLine {
  mark: "+" | "-" | " ";
  label: string;
  text: string;
}

export interface BodyProblem {
  kind: "not_text" | "unreadable" | "not_json";
  reason: string;
}

export interface DiffRefusal {
  previous: BodyProblem | null;
  current: BodyProblem | null;
}

/** Что одна сторона диффа несёт в слоте абзаца (rework R1). */
export type DiffParagraphState = "stored" | "pre_protocol" | "unreadable";

export interface SpecDiff {
  paragraph: DiffLine[];
  requirements: DiffLine[];
  /** Состояние абзаца прошлой версии: свой абзац / до протокола / тело не читается. */
  paragraph_previous: DiffParagraphState;
  /** То же для текущей версии. */
  paragraph_current: DiffParagraphState;
  /** Почему своего абзаца нет: сторона названа; бизнес-описание за абзац не выдаётся. */
  paragraph_note: string | null;
  /** Заполнено, когда сравнение отказано: непрочитанная версия — не пустая. */
  refusal: DiffRefusal | null;
  note: string | null;
}

export interface PreviousView {
  artifact: Artifact;
  diff: SpecDiff;
}

export interface SignatureFact {
  kind: "approval" | "rejection";
  actor: string;
  at: string;
  rationale: string | null;
  spec_seq: number | null;
  source: "decision" | "event";
}

export interface SignatureView {
  task_id: string;
  state: string;
  snapshot_at: string;
  may_approve: boolean;
  may_approve_denied: string | null;
  may_reject: boolean;
  may_reject_denied: string | null;
  spec: SpecView | null;
  review: ReviewView | null;
  review_note: string | null;
  previous: PreviousView | null;
  previous_note: string | null;
  signature: SignatureFact | null;
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

// --- homes registry (REQ-HOME-01…09) ---
// HomeIssue — типизированная причина недоступности дома на проводе
// (R2 ревью, rel/error-context): code — машинный токен, по которому UI
// ветвит (home_unavailable / home_no_store / home_schema_version), path —
// дом, о котором вердикт, hint — подсказка платформы, message — текст для
// человека. UI не перефразирует message и не выводит code из текста.
export interface HomeIssue {
  code: string;
  path: string;
  message: string;
  hint: string | null;
}

// HomeRecord — хранимая запись реестра + атрибуты, снятые операцией
// перечисления (доступность — единственный read-time probe). version_note
// и read_error — данные платформы, UI их не перефразирует.
export interface HomeRecord {
  path: string;
  project: string;
  // project_source — откуда взята метка проекта (R3): "store" — из проектов
  // самого дома, "path" — стор прочитан, проектов нет (метка из формы
  // пути), "unreadable" — стор не прочитался (причина в project_error),
  // "unknown" — запись старше этого маркера.
  project_source: "store" | "path" | "unreadable" | "unknown";
  project_error: string | null;
  added_at: string;
  enabled: boolean;
  active: boolean;
  available: boolean;
  schema_version: number | null;
  version_note: string | null;
  read_error: HomeIssue | null;
}

// Блок совмещённого взгляда: одна запись = один включённый дом,
// атрибуция в каждом блоке, записи прочитаны live из дома-источника.
export interface HomeRecordsView<T> {
  snapshot_at: string;
  home_path: string;
  home_project: string;
  is_active: boolean;
  available: boolean;
  version_note: string | null;
  error: HomeIssue | null;
  records: T[];
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

// --- доступ с телефона (task 01a1be98, REQ-LAN-01…09) ---

// Одно подключённое устройство. `id` — идентификатор сессии (по нему окно
// может отключить ОДИН телефон), `live` — был ли контакт за последнюю минуту
// (ratified Q5); сама сессия при молчании не рвётся (ratified Q3).
export interface LanDevice {
  id: string;
  name: string;
  addr: string;
  connected_at: string;
  last_seen: string;
  live: boolean;
}

// Фактическое состояние доступа. `state` — не намерение, а факт:
// off | awaiting (сервер поднят, телефон ещё не подключён) | connected.
// Токены объявлены один раз в ./lanState (LAN_STATES) и сверяются
// cargo-тестом с Rust-перечислением.
export interface LanStatus {
  state: LanState;
  address: string | null;
  /// Действующее (не использованное) приглашение существует.
  invite_active: boolean;
  /// Адрес для браузера (текстом, его можно открыть или скопировать).
  url: string | null;
  /// Тот же доступ ссылкой приложения `devai://pair` — её кодирует QR, чтобы
  /// телефон отдал сканирование нашему APK (REQ-QRAPP-01/02). Строится из того
  /// же кода и того же адреса, что `url`.
  app_url: string | null;
  token: string | null;
  qr_svg: string | null;
  /// QR не построился — это НЕ то же самое, что «код уже использован».
  qr_error: string | null;
  reason_code: string | null;
  reason: string | null;
  /// Не удалось сохранить флаг «был включён»: следующего запуска это
  /// коснётся, и окно обязано сказать об этом.
  persist_error: string | null;
  devices: LanDevice[];
  started_at: string | null;
}

// --- HTTP/JSON-поверхность телефона (task 01a1be98, quality return R9) ---
// Зеркало конверта POST /rpc: {command, params} → {ok, data | error}.
// Паритет с Rust-константами RPC_ENVELOPE_FIELDS закреплён cargo-тестом
// `the_rpc_envelope_is_registered_and_mirrored`.

export interface RpcRequest {
  command: string;
  params: Record<string, unknown>;
}

export interface RpcResponse<T> {
  ok: boolean;
  data?: T;
  error?: UiError;
}
