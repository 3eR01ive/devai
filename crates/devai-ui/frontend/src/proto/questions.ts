// Гейт вопроса читается по СОБЫТИЮ, а не по формулировке вопроса: слово в
// тексте — текст, а адрес перехода — данные платформы.
import type { WorkflowEvent } from "../types";

export const GATE_REVIEW_RETURN = "review-return";
/// Вопрос, поднятый пределом попыток: платформа принимает РОВНО два слова из
/// своего словаря, поэтому свободный текст здесь — гарантированный отказ.
export const GATE_ATTEMPT_LIMIT = "attempt-limit";

export interface AttemptTrip {
  role: string;
  attempt: number;
  limit: number;
}

/** Данные остановки по пределу попыток из события `attempt.limit_tripped`. */
export function attemptTrip(events: WorkflowEvent[], questionId: string): AttemptTrip | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.kind !== "attempt.limit_tripped") continue;
    const p = (e.payload ?? {}) as Record<string, unknown>;
    if (String(p.question_id ?? "") !== questionId) continue;
    return {
      role: String(p.role ?? ""),
      attempt: Number(p.attempt ?? 0),
      limit: Number(p.limit ?? 0),
    };
  }
  return null;
}

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
