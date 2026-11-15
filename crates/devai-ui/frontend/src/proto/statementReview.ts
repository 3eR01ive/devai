// Протокол проверки постановки (`statement-review@1`) — это ДАННЫЕ: вердикт,
// абзац человеку, чек-лист из шести пунктов и список вопросов. Экран не
// печатает склейку этих полей как свободный текст, а раскладывает их по местам.
export interface StatementCheck {
  item: string;
  ok: boolean;
  note?: string | null;
}

export interface StatementReview {
  verdict: string;
  human_paragraph: string;
  checklist: StatementCheck[];
  questions: string[];
}

// Имена пунктов — из схемы контракта (enum), слова — для человека.
const ITEM_WORD: Record<string, string> = {
  user: "пользователь",
  observable: "наблюдаемый результат",
  boundaries: "границы",
  volume: "объём",
  acceptance: "критерий приёмки",
  single_milestone: "одна веха",
};

export function itemWord(item: string): string {
  return ITEM_WORD[item] ?? item;
}

/** Разбор тела артефакта проверки постановки; null — это не протокол. */
export function parseStatementReview(body: unknown): StatementReview | null {
  if (!body || typeof body !== "object") return null;
  const o = body as Record<string, unknown>;
  const verdict = typeof o.verdict === "string" ? o.verdict : "";
  if (!verdict) return null;
  const checks = Array.isArray(o.checklist)
    ? (o.checklist as Record<string, unknown>[])
        .filter((c) => c && typeof c.item === "string")
        .map((c) => ({
          item: String(c.item),
          ok: c.ok === true,
          note: typeof c.note === "string" ? c.note : null,
        }))
    : [];
  const questions = Array.isArray(o.questions)
    ? (o.questions as unknown[]).map((q) => String(q))
    : [];
  return {
    verdict,
    human_paragraph: typeof o.human_paragraph === "string" ? o.human_paragraph : "",
    checklist: checks,
    questions,
  };
}
