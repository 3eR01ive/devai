// Слова раздела связок (REQ-MS-06/07). Экран показывает РОВНО одно слово
// статуса и НИКОГДА не выводит причину, которой нет в доказательстве.
//
// Словарь «код→слово» живёт в ОДНОМ месте — `devai_core::types` (R3): API
// отдаёт готовые `state_text` / `reason_text` / `freshness_text` /
// `verdict_text`, и окно только печатает их. Здесь остаются подписи ПУСТЫХ
// полей связки (подстановки значений нет) и механика удержания кнопки.
// Никакой логики приоритета доказательств в окне нет — она в платформе
// (no read-time rederivation).

/** Пустые поля связки подписаны словами, подстановки нет (REQ-MS-01 AC2). */
export function fieldWord(value: string, empty: string): string {
  return value.trim() === "" ? empty : value;
}

export function providerWord(value: string): string {
  // `local` is the neutral `agent add` default and means the SAME as empty:
  // the driver chooses (REQ-MS-01 AC2) — the value is never shown as a
  // provider name.
  return fieldWord(value, "не задан — выбирает драйвер").replace(
    /^local$/,
    "не задан — выбирает драйвер",
  );
}

export function modelWord(value: string): string {
  return fieldWord(value, "не передаётся");
}

/** Уровень размышления — часть связки (REQ-MS-09). Пустой уровень значит
 * неполную связку, а не «водитель решит сам»; подстановки нет. */
export function reasoningWord(value: string): string {
  return fieldWord(value, "не задан — связка неполна");
}

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
