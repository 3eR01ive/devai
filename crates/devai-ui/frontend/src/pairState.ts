// Причины отказа в приглашении — ОДНА декларация и ОДНА презентация для
// приложения и страницы (task 01a1bf78, REQ-QRAPP-06).
//
// Список кодов и сами фразы объявлены в Rust
// (`crates/devai-ui/src/lan/pair.rs::PairRefusal`) — там же, где решается,
// какая причина случилась; cargo-тест
// `the_refusal_vocabulary_is_mirrored_in_the_frontend` сверяет этот файл с
// ним. Поэтому «просрочено», «не та сеть» и «доступ закрыт» не могут
// слиться в одно общее «ошибка связи»: три состояния — три разных слова.
//
// Слова НЕ переписываются на лету: экран показывает ровно ту строку, которая
// пришла из единственного источника.

export const PAIR_REASONS = [
  "invite_expired",
  "wrong_network",
  "access_closed",
  "pair_bad_link",
] as const;

export type PairReason = (typeof PAIR_REASONS)[number];

export interface PairRefusalView {
  /// Человеческая причина — та же строка, что печатает Rust.
  title: string;
  /// Что делать дальше: экран отказа не тупик.
  hint: string;
}

export const PAIR_REFUSALS: Record<PairReason, PairRefusalView> = {
  invite_expired: {
    title: "приглашение просрочено",
    hint:
      "Приглашение одноразовое: его уже использовали, заменили новым или доступ " +
      "выключили. Выпустите новое приглашение на машине и отсканируйте его.",
  },
  wrong_network: {
    title: "не та сеть",
    hint:
      "Телефон не в сети этой машины. Вернитесь в домашнюю сеть машины и откройте " +
      "приглашение снова.",
  },
  access_closed: {
    title: "доступ закрыт",
    hint:
      "Доступ с телефона выключили или это устройство отключили на машине. " +
      "Для возврата нужно новое приглашение.",
  },
  pair_bad_link: {
    title: "ссылка не подходит",
    hint:
      "Это не приглашение devai: ссылка испорчена, ведёт наружу или в локальный " +
      "файл. Отсканируйте код из окна машины.",
  },
};

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
export const PAIR_NOTICES = {
  pair_not_remembered: {
    title: "дом не запомнится",
    hint:
      "Подключение работает, но адрес дома не сохранился на телефоне: при " +
      "следующем запуске приложение его не откроет. Приглашение можно " +
      "отсканировать снова, когда понадобится.",
  },
} as const;

export type PairNoticeCode = keyof typeof PAIR_NOTICES;

export function isPairNotice(value: string | null | undefined): value is PairNoticeCode {
  return !!value && value in PAIR_NOTICES;
}

/// Заметка из адреса страницы; неизвестный код — не повод придумывать текст.
export function pairNotice(code: string | null | undefined): PairRefusalView | null {
  return isPairNotice(code) ? PAIR_NOTICES[code] : null;
}
