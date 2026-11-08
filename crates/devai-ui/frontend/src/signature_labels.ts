// Слова секции подписи (REQ-SIGN-02 AC4/AC5, ASM-16, rework R5/R8).
//
// Это НЕ общий i18n и НЕ словарь чипа состояния: `StateChip.vue` печатает
// хранимый токен как есть, а `states.ts` — зеркало токенов TaskState; оба
// остаются вне этой поставки.
//
// Сами слова — ДАННЫЕ: единственный источник
// `crates/devai-api/src/signature/labels.json`. Модуль
// `src/generated/signature-labels.ts` — его рендер
// (scripts/gen-signature-labels.mjs), а Rust-тесты `signature/vocabulary.rs`
// сверяют наборы токенов файла с закрытыми перечислениями. Здесь только
// чтение рендера и правило «слова нет — токен печатается как есть с
// пометкой»; ручных карт и ручных списков токенов в коде больше нет.
import {
  ARTIFACT_STATUS_LABELS,
  DENIAL_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  REVIEW_VERDICT_LABELS,
  SIGNATURE_KIND_LABELS,
  SIGNATURE_SOURCE_LABELS,
} from "./generated/signature-labels.ts";

export {
  ARTIFACT_STATUS_LABELS,
  DENIAL_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  REVIEW_VERDICT_LABELS,
  SIGNATURE_KIND_LABELS,
  SIGNATURE_SOURCE_LABELS,
};

export interface Label {
  text: string;
  /** true — слова нет в словаре, токен напечатан как есть с пометкой. */
  raw: boolean;
}

/// Слово для токена; нераспознанный токен печатается как есть с пометкой.
export function labelFor(map: Record<string, string>, token: string | null | undefined): Label {
  if (token === null || token === undefined || token === "") {
    return { text: "—", raw: false };
  }
  const word = map[token];
  return word ? { text: word, raw: false } : { text: `${token} (слово не задано)`, raw: true };
}

/// Наборы, которые словарь обязан покрывать, — это ключи рендера, а не
/// рукописный список: набор, добавленный в данные, существует здесь по
/// построению, а полноту относительно типов проверяют Rust-тесты.
export const CLOSED_TOKEN_SETS: Record<string, string[]> = Object.fromEntries(
  Object.entries({
    ARTIFACT_STATUS_LABELS,
    REVIEW_VERDICT_LABELS,
    PARAGRAPH_SOURCE_LABELS,
    REVIEW_PARAGRAPH_SOURCE_LABELS,
    DENIAL_LABELS,
    SIGNATURE_KIND_LABELS,
    SIGNATURE_SOURCE_LABELS,
  }).map(([name, map]) => [name, Object.keys(map)]),
);
