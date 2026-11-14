// Разбор вопроса из документа и сборка rationale подписи.
// Чистые функции: экран только раскладывает, правила читаются тестами.
//
// Вопрос аналитика обычно несёт предложение («…Предлагаю: …»). Человек правит
// ПРЕДЛОЖЕНИЕ, а вопрос остаётся вопросом — поэтому они разбираются здесь,
// а не в шаблоне.

const OQ_SPLIT = /\s*(?:Предлагаю|Предложение|Предлагаемый ответ|Решение)\s*[:—]\s*/i;

function asText(v: unknown): string {
  if (v == null) return "";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(asText).join(" ");
  if (typeof v === "object") {
    const o = v as Record<string, unknown>;
    for (const k of ["text", "question", "statement"]) if (typeof o[k] === "string") return o[k] as string;
    try { return JSON.stringify(v); } catch { return String(v); }
  }
  return String(v);
}

/** Вопрос отдельно, предложенное решение отдельно. */
export function oqParts(q: unknown): { question: string; proposal: string } {
  const s = asText(q).trim();
  const m = OQ_SPLIT.exec(s);
  if (!m || m.index <= 0) return { question: s, proposal: "" };
  return { question: s.slice(0, m.index).trim(), proposal: s.slice(m.index + m[0].length).trim() };
}

/** Решения человека → rationale подписи: одна строка на вопрос плюс заметка. */
export function buildRationale(
  list: { k: number; question: string; proposal: string }[],
  picked: Record<number, string>,
  extra: string,
): string | null {
  const parts = list
    .map((q) => ({ n: q.k + 1, text: (picked[q.k] ?? "").trim() }))
    .filter((x) => x.text.length)
    .map((x) => `Вопрос ${x.n}: ${x.text}`);
  const tail = extra.trim();
  if (tail) parts.push(tail);
  return parts.length ? parts.join("\n") : null;
}
