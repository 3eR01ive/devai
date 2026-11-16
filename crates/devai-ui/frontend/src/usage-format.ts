// ОДНА сборка строки расхода токенов для всех поверхностей (task 01a1b851,
// REQ-USG-07/08/09): карточка задачи, строка списка и общий экран берут слова
// отсюда, а не каждая свой вариант. Три копии форматтера уже расходились
// (одна печатала "" для нуля, другая "0", третья "—" для неизвестного), и
// оператор видел три разных ответа на один и тот же расход (mt/dry).
//
// Числа приходят из одной операции чтения (`task.usage` / `usage.summary`) —
// здесь только запись числа, никакой арифметики по прогонам.
import type { TaskUsageView } from "./types";

/** Компактная запись числа токенов. `null`/`undefined` — «неизвестно» («—»),
 *  НЕ ноль: отсутствие цифр не выдаётся за бесплатный прогон. */
export function tok(n: number | null | undefined): string {
  if (n == null) return "—";
  if (n >= 1e9) return (n / 1e9).toFixed(2) + " млрд";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " млн";
  if (n >= 1e3) return Math.round(n / 1e3) + " тыс";
  return String(n);
}

/** Расход: сумма известных прогонов. Когда известных нет, а прогоны с
 *  НЕИЗВЕСТНЫМ расходом есть, печатается «—», а не «0»; число таких прогонов
 *  стоит рядом (`usage.unknown`). */
export function spentText(spent: number, unknown: number): string {
  if (spent === 0 && unknown > 0) return "—";
  return tok(spent);
}

/** Правая половина строки: «из …» или «без лимита» словами — НЕ «0 из 0». */
export function limitText(u: TaskUsageView): string {
  return u.limit == null ? "без лимита" : "из " + tok(u.limit);
}

/** Строка расхода задачи — карточка и список собирают её ОДНИМ кодом, поэтому
 *  разойтись в словах не могут. */
export function taskSpendLine(u: TaskUsageView): string {
  return `${spentText(u.spent, u.unknown)} ${limitText(u)}`;
}

/** Стоимость: число, когда оно известно; «неизвестна» — когда прогоны есть, а
 *  цены нет; «—» — когда и прогонов нет. */
export function money(v: number, measured: number): string {
  if (v > 0) return "$" + v.toFixed(2);
  return measured > 0 ? "неизвестна" : "—";
}
