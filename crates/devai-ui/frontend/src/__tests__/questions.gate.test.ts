// Гейт вопроса берётся из события `question.raised`, а не из текста вопроса:
// «ревью вернуло спеку» — это данные перехода, а не слово в формулировке.
import { describe, expect, it } from "vitest";

import { gateOf, GATE_REVIEW_RETURN } from "../proto/questions";
import type { WorkflowEvent } from "../types";

const ev = (kind: string, payload: unknown): WorkflowEvent =>
  ({ id: kind + Math.random(), task_id: "t", kind, actor: "a", payload, created_at: "" });

describe("gateOf", () => {
  it("находит гейт по question_id", () => {
    const events = [
      ev("spec.published", {}),
      ev("question.raised", { question_id: "q1", gate: GATE_REVIEW_RETURN }),
    ];
    expect(gateOf(events, "q1")).toBe("review-return");
  });

  it("чужой вопрос гейта не получает", () => {
    const events = [ev("question.raised", { question_id: "q1", gate: GATE_REVIEW_RETURN })];
    expect(gateOf(events, "q2")).toBeNull();
  });

  it("последнее событие побеждает", () => {
    const events = [
      ev("question.raised", { question_id: "q1", gate: "other" }),
      ev("question.raised", { question_id: "q1", gate: GATE_REVIEW_RETURN }),
    ];
    expect(gateOf(events, "q1")).toBe("review-return");
  });

  it("событие без гейта — null, а не догадка", () => {
    expect(gateOf([ev("question.raised", { question_id: "q1" })], "q1")).toBeNull();
  });
});
