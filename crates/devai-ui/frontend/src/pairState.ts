// Причины отказа и заметки приглашения — ОДИН источник для приложения и
// страницы (task 01a1bf78, REQ-QRAPP-06; качество R5).
//
// Слова объявлены в Rust (`crates/devai-ui/src/lan/pair.rs`, таблицы REFUSALS
// и NOTICES), а этот модуль лишь переэкспортирует СГЕНЕРИРОВАННЫЙ из них файл
// `./generated/pair-vocabulary`. Рукописного зеркала больше нет: текст нельзя
// поправить в одном месте и забыть в другом. Поэтому «просрочено», «не та
// сеть» и «доступ закрыт» не могут слиться в одно общее «ошибка связи»: три
// состояния — три разных слова и три разных машинных кода.
//
// НЕ РЕДАКТИРУЙТЕ тексты здесь: правка живёт в Rust и приезжает через
// `node scripts/gen-pair-vocabulary.mjs` (его же проверяет `npm test`).
import {
  PAIR_VOCABULARY,
  type PairNoticeCode,
  type PairReason,
} from "./generated/pair-vocabulary";

export type { PairNoticeCode, PairReason };

export interface PairRefusalView {
  /// Человеческая причина — та же строка, что печатает Rust.
  title: string;
  /// Что делать дальше: экран отказа не тупик.
  hint: string;
}

/// Все причины, которые поверхность умеет показать (порядок — как в Rust).
export const PAIR_REASONS = Object.keys(PAIR_VOCABULARY.refusals) as PairReason[];

export const PAIR_REFUSALS: Record<PairReason, PairRefusalView> = PAIR_VOCABULARY.refusals;

export function isPairReason(value: string | null | undefined): value is PairReason {
  return !!value && (PAIR_REASONS as readonly string[]).includes(value);
}

/// Причина по коду из адреса (`#/pair?reason=…`); неизвестный код — не повод
/// выдумывать свою фразу.
export function pairRefusal(reason: string | null | undefined): PairRefusalView | null {
  return isPairReason(reason) ? PAIR_REFUSALS[reason] : null;
}

// ---------------------------------------------------------------------------
// Заметки, которые приложение оставляет человеку ПОСЛЕ перехода на дом
// ---------------------------------------------------------------------------
// Отказ — это «не пустили»; заметка — «пустили, но вот что не сохранилось».
// Код едет в адресе открытой страницы (`?notice=…`), поэтому заметка видна
// уже на интерфейсе машины, а не мигает на служебной странице приложения
// (качество: R1 — проглоченная ошибка записи дома).
export const PAIR_NOTICES: Record<PairNoticeCode, PairRefusalView> = PAIR_VOCABULARY.notices;

export function isPairNotice(value: string | null | undefined): value is PairNoticeCode {
  return !!value && value in PAIR_NOTICES;
}

/// Заметка из адреса страницы; неизвестный код — не повод придумывать текст.
export function pairNotice(code: string | null | undefined): PairRefusalView | null {
  return isPairNotice(code) ? PAIR_NOTICES[code] : null;
}
