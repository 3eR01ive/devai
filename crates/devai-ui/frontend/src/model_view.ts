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

/** Удержание удаления (REQ-MC-05 AC7): та же механика, отдельный порог не
 * нужен — но и здесь обработчика click нет, поэтому одиночный и двойной
 * клик ничего не удаляют. */
export const HOLD_TO_DELETE_MS = HOLD_TO_SWITCH_MS;

/**
 * Идентификатор новой связки из имени (ASM-9) — ЗЕРКАЛО
 * `devai_store::bundle_id_from_label`: нижний регистр, латиница/цифры,
 * одиночные дефисы. Показывается в форме ДО сохранения; после создания не
 * меняется. Пустая строка = из имени ничего не выводится (сервер откажет с
 * тем же смыслом — второй реализации правила записи здесь нет, только
 * предпросмотр).
 */
export function bundleIdFromLabel(label: string): string {
  let out = "";
  let dash = false;
  for (const ch of label.toLowerCase()) {
    if (/[a-z0-9]/.test(ch)) {
      out += ch;
      dash = false;
    } else if (/[-_ .·/]/.test(ch) && out.length > 0 && !dash) {
      out += "-";
      dash = true;
    }
  }
  return out.replace(/-+$/, "");
}
