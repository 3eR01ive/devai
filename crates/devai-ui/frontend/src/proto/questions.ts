// Гейт вопроса читается по СОБЫТИЮ, а не по формулировке вопроса: слово в
// тексте — текст, а адрес перехода — данные платформы.
import type { WorkflowEvent } from "../types";

export const GATE_REVIEW_RETURN = "review-return";

export function gateOf(events: WorkflowEvent[], questionId: string): string | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.kind !== "question.raised") continue;
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (String(p.question_id ?? "") !== questionId) continue;
    return typeof p.gate === "string" ? p.gate : null;
  }
  return null;
}
