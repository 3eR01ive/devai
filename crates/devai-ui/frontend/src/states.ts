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
