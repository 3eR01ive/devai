// Зеркало хранимых токенов TaskState Rust (devai-core/src/types.rs,
// str_enum!(TaskState, ...)). UI сравнивает токены как ДАННЫЕ — общий
// словарь, чтобы в шаблонах не жили сырые литералы.
// Пин: тест states_ts_mirrors_rust_task_state_tokens (devai-ui/src/commands.rs)
// сверяет каждый вариант с этим файлом; переименование токена в Rust
// ломает cargo test, а не даёт молчаливый undefined в окне.
export const TaskStateToken = {
  Draft: "DRAFT",
  Analysis: "ANALYSIS",
  Review: "REVIEW",
  Approval: "APPROVAL",
  Implementation: "IMPLEMENTATION",
  Qa: "QA",
  Rework: "REWORK",
  WaitingForInput: "WAITING_FOR_INPUT",
  Paused: "PAUSED",
  Blocked: "BLOCKED",
  Failed: "FAILED",
  Accepted: "ACCEPTED",
  Cancelled: "CANCELLED",
} as const;

export type TaskStateToken =
  (typeof TaskStateToken)[keyof typeof TaskStateToken];

// Зеркало ExecutionStatus (devai-core/src/types.rs, str_enum!(ExecutionStatus,
// ...)): тот же пин-приём — фильтр по статусу исполнения сравнивает ТОКЕН из
// словаря, а не литерал в шаблоне (иначе переименование в Rust молча даёт
// пустой список вместо ошибки сборки).
export const ExecutionStatusToken = {
  Running: "RUNNING",
  Completed: "COMPLETED",
  Failed: "FAILED",
  Cancelled: "CANCELLED",
} as const;

export type ExecutionStatusToken =
  (typeof ExecutionStatusToken)[keyof typeof ExecutionStatusToken];

// Ответ проверки живости (devai-runtime/src/liveness.rs, Liveness::as_str):
// alive | dead | unknown. «Мёртво» — только доказанная смерть (REQ-LV-02);
// «проверить нельзя» отдельное слово и никогда не death.
export const LivenessToken = {
  Alive: "alive",
  Dead: "dead",
  Unknown: "unknown",
} as const;

export type LivenessToken =
  (typeof LivenessToken)[keyof typeof LivenessToken];
