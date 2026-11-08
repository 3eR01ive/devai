// Чистые правила секции подписи, вынесенные из шаблона: их проверяют тесты
// (REQ-SIGN-11 AC4 «UI-ветвление — отдельной функцией/компонентом»,
// REQ-SIGN-04 «отпускание раньше порога отменяет — порог именованная
// константа»). Компонент только раскладывает готовые строки.

import type { ReviewView, SpecView } from "./types";
import {
  DENIAL_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  REVIEW_VERDICT_LABELS,
  labelFor,
} from "./signature_labels";

/// Порог удержания кнопки подписи (OQ-1: число не зафиксировано спекой —
/// до ответа оператора это ИМЕНОВАННАЯ константа, не магия в шаблоне).
export const HOLD_MS = 700;

/// Удержание завершилось? Отпускание раньше порога мутацию не отправляет.
export function holdCompletes(elapsedMs: number): boolean {
  return elapsedMs >= HOLD_MS;
}

/// Первая причина отказа словами (не общее «нельзя»): токен из API.
export function denialText(token: string | null): string | null {
  if (!token) return null;
  return labelFor(DENIAL_LABELS, token).text;
}

/// Ветка абзаца ревьювера (REQ-SIGN-11): каждая ветка даёт видимый текст,
/// ни одна — пустой блок. `review=null` (ветка 1) обрабатывает вызывающий
/// через `review_note`.
export function reviewParagraph(review: ReviewView): {
  text: string;
  sourceNote: string;
  note: string | null;
} {
  const source = labelFor(REVIEW_PARAGRAPH_SOURCE_LABELS, review.paragraph_source);
  const text =
    review.human_paragraph ??
    "абзаца ревьювера нет — печатать нечего (вердикт сохранён как есть)";
  return {
    text,
    sourceNote: source.text,
    note: review.note,
  };
}

/// Вердикт — сохранённый токен, переведённый словом рядом; сам токен не
/// подменяется «одобрено» без него (REQ-SIGN-11 AC2).
export function reviewVerdict(review: ReviewView): { token: string; word: string } {
  const token = review.verdict ?? "—";
  return { token, word: labelFor(REVIEW_VERDICT_LABELS, review.verdict).text };
}

/// Абзац спеки и честная метка его происхождения.
export function specParagraph(spec: SpecView): { text: string; sourceNote: string; note: string | null } {
  return {
    text:
      spec.human_paragraph ??
      "абзац и бизнес-описание пусты — предъявить нечего",
    sourceNote: labelFor(PARAGRAPH_SOURCE_LABELS, spec.paragraph_source).text,
    note: spec.unavailable_note,
  };
}

/// Обоснование сохранённого факта для печати (REQ-SIGN-06 AC2): null И
/// пустая строка — один и тот же факт «без обоснования». `Some("")` достижим
/// через CLI (`task approve --rationale ""`), поэтому панель не должна
/// печатать пустой абзац под меткой обоснования; непустая строка печатается
/// как сохранена.
export function signatureRationale(rationale: string | null): string | null {
  if (rationale === null || rationale.trim() === "") return null;
  return rationale;
}

/// Счёт раскрывающихся критериев требования: «3 AC · 1 ограничение».
export function criteriaSummary(ac: number, constraints: number): string {
  const parts: string[] = [];
  if (ac > 0) parts.push(`${ac} критери${ac === 1 ? "й" : ac < 5 ? "я" : "ев"}`);
  if (constraints > 0) parts.push(`${constraints} ограничени${constraints === 1 ? "е" : "я"}`);
  return parts.length ? parts.join(" · ") : "критериев нет";
}

export interface OqChoice {
  /** «принять как есть» по умолчанию. */
  accept: boolean;
  answer: string;
}

/// Строки обоснования от per-OQ выбора (REQ-SIGN-03): выбранное положение
/// попадает в ВИДИМОЕ правимое поле; сервер ничего не пересобирает.
export function oqRationaleLines(choices: OqChoice[]): string[] {
  return choices
    .map((choice, i) =>
      choice.accept
        ? `Вопрос ${i + 1}: принято как есть`
        : `Вопрос ${i + 1}: ${choice.answer.trim()}`,
    )
    .filter((line, i) => (choices[i].accept ? true : choices[i].answer.trim() !== ""));
}

/// Подпись доступна, только когда обоснование непустое (REQ-SIGN-04 AC4):
/// пустая строка и строка из пробелов не подписывают и не отклоняют.
export function rationaleValid(rationale: string): boolean {
  return rationale.trim().length > 0;
}

/// Каждому «своему ответу» нужен непустой текст, иначе подпись недоступна.
export function choicesValid(choices: OqChoice[]): boolean {
  return choices.every((c) => c.accept || c.answer.trim().length > 0);
}

/// Обновить обоснование строками выбора, сохранив ручной текст оператора:
/// прежние строки «Вопрос N: …» заменяются, остальные остаются как есть.
export function mergeOqLines(rationale: string, choices: OqChoice[]): string {
  const manual = rationale
    .split("\n")
    .filter((line) => !/^Вопрос \d+:/.test(line))
    .join("\n")
    .trim();
  const oq = oqRationaleLines(choices).join("\n");
  return [manual, oq].filter((s) => s !== "").join("\n");
}
