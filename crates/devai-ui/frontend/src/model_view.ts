// Слова раздела связок (REQ-MS-06/07). Экран показывает РОВНО одно слово
// статуса и НИКОГДА не выводит причину, которой нет в доказательстве.
//
// Словарь «код→слово» живёт в ОДНОМ месте — `devai_core::types` (R3/R5): API
// отдаёт готовые `state_text` / `reason_text` / `freshness_text` /
// `verdict_text`, а для полей связки — `llm_provider_text` / `model_text` /
// `reasoning_text`. Окно только печатает их: своей копии словаря здесь нет
// (R5 quality return), поэтому пустые поля больше не переписываются на фронте.
//
// Здесь остаётся только механика удержания кнопки. Никакой логики приоритета
// доказательств в окне нет — она в платформе (no read-time rederivation).

/** Удержание кнопки применения (REQ-MS-07): без модального подтверждения,
 * одиночный и двойной клик ничего не применяют — обработчика click нет. */
export const HOLD_TO_SWITCH_MS = 700;

/** Timer grace: a `setTimeout` may fire a fraction of a millisecond early
 * (ms truncation), and `holdCompletes` would then reject a hold the operator
 * did complete. The grace keeps the FIRED timer at or above the threshold. */
export const HOLD_TIMER_GRACE_MS = 5;

export function holdCompletes(elapsedMs: number): boolean {
  return elapsedMs >= HOLD_TO_SWITCH_MS;
}
