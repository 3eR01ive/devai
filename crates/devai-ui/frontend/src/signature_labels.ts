// Подписи токенов ДЛЯ СЕКЦИИ ПОДПИСИ (REQ-SIGN-02 AC4/AC5, ASM-16).
//
// Это НЕ общий i18n и НЕ словарь чипа состояния: `StateChip.vue` печатает
// хранимый токен как есть, а `states.ts` — зеркало токенов TaskState; оба
// остаются вне этой поставки. Здесь — одна константа модуля секции, которая
// покрывает РОВНО те закрытые наборы, которые секция действительно показывает:
//   * ArtifactStatus: CURRENT / STALE / SUPERSEDED (types.rs:492-501);
//   * вердикт отчёта ревьювера — в написании отчёта (approved /
//     needs_revision / rejected);
//   * paragraph_source спеки: spec / business_description;
//   * paragraph_source ревьювера: review_report / summary / none;
//   * коды отказов may_approve_denied / may_reject_denied: not_approval /
//     no_spec / spec_not_current / no_requirements_version;
//   * вид сохранённого факта подписи: approval / rejection.
// Слово для токена, которого здесь нет, НЕ выдумывается: токен печатается
// как есть с пометкой (labelFor).

export const ARTIFACT_STATUS_LABELS: Record<string, string> = {
  CURRENT: "действует",
  STALE: "устарел",
  SUPERSEDED: "заменён",
};

export const REVIEW_VERDICT_LABELS: Record<string, string> = {
  approved: "одобрено",
  needs_revision: "нужны правки",
  rejected: "отклонён",
};

export const PARAGRAPH_SOURCE_LABELS: Record<string, string> = {
  spec: "абзац спеки",
  business_description: "бизнес-описание задачи",
};

export const REVIEW_PARAGRAPH_SOURCE_LABELS: Record<string, string> = {
  review_report: "абзац отчёта ревьювера",
  summary: "summary отчёта (абзаца нет)",
  none: "абзаца нет",
};

export const DENIAL_LABELS: Record<string, string> = {
  not_approval: "подпись доступна только в состоянии APPROVAL",
  no_spec: "спеки нет — утверждать нечего",
  spec_not_current: "последняя спека не CURRENT — утверждать нельзя",
  no_requirements_version: "у задачи нет утверждённой версии требований",
};

export const SIGNATURE_KIND_LABELS: Record<string, string> = {
  approval: "утверждение",
  rejection: "отклонение",
};

export const SIGNATURE_SOURCE_LABELS: Record<string, string> = {
  decision: "строка решения",
  event: "событие (сохранённой строки решения нет)",
};

/// Состояние абзаца в диффе спеки (rework R1): сторона без своего абзаца
/// называется словом, а не выдаётся за пустой абзац.
export const DIFF_PARAGRAPH_STATE_LABELS: Record<string, string> = {
  stored: "свой абзац",
  pre_protocol: "абзаца нет (спека до протокола)",
  unreadable: "тело не читается",
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

/// Полные наборы, которые словарь обязан покрывать. Тест падает, если
/// набор расширили (например новый ArtifactStatus), а слово забыли.
export const CLOSED_TOKEN_SETS: Record<string, string[]> = {
  ARTIFACT_STATUS_LABELS: ["CURRENT", "STALE", "SUPERSEDED"],
  REVIEW_VERDICT_LABELS: ["approved", "needs_revision", "rejected"],
  PARAGRAPH_SOURCE_LABELS: ["spec", "business_description"],
  REVIEW_PARAGRAPH_SOURCE_LABELS: ["review_report", "summary", "none"],
  DENIAL_LABELS: [
    "not_approval",
    "no_spec",
    "spec_not_current",
    "no_requirements_version",
  ],
  SIGNATURE_KIND_LABELS: ["approval", "rejection"],
  SIGNATURE_SOURCE_LABELS: ["decision", "event"],
  DIFF_PARAGRAPH_STATE_LABELS: ["stored", "pre_protocol", "unreadable"],
};
