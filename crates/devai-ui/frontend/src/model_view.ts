// Слова раздела связок (REQ-MS-06/07) — один закрытый словарь для экрана и
// CLI-зеркала. Экран показывает РОВНО одно слово статуса и НИКОГДА не
// выводит причину, которой нет в доказательстве.
//
// Почему отдельным модулем: статус — вычисляемое поле снимка (state), а не
// два индикатора; здесь только перевод токена в слово. Никакой логики
// приоритета доказательств в окне нет — она в платформе (no read-time
// rederivation).

/** Ровно одно слово статуса — по полю state снимка. */
export function statusWord(state: string): string {
  switch (state) {
    case "live":
      return "живая";
    case "not_working":
      return "не работает";
    case "not_proven":
      return "не доказано";
    case "not_selected":
      return "связка не выбрана";
    default:
      return "неизвестно";
  }
}

/** Причина называется только когда доказана; иначе — честный отказ. */
export function reasonWord(code: string | null): string {
  if (!code) return "причина не распознана";
  switch (code) {
    case "no_credits":
      return "нет кредитов";
    case "model_not_found":
      return "модель не существует";
    case "no_key":
      return "нет ключа провайдера";
    case "network":
      return "сеть недоступна";
    case "provider":
      return "отказ провайдера";
    case "причина не классифицирована":
      return "причина не классифицирована";
    case "manual":
    case "limits":
    case "error":
      return code;
    default:
      return "причина не распознана";
  }
}

/** Свежесть доказательства — как её отдал снимок, без пересчёта в окне. */
export function freshnessWord(value: string): string {
  switch (value) {
    case "fresh":
      return "свежая проба";
    case "stale":
      return "проба устарела";
    default:
      return "доказательств нет";
  }
}

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

export function holdCompletes(elapsedMs: number): boolean {
  return elapsedMs >= HOLD_TO_SWITCH_MS;
}
