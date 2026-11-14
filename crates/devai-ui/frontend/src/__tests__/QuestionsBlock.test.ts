// Вопрос платформы: без ответа человека работа стоит, поэтому кнопки есть в
// карточке, а не только в консоли. Гейт вопроса — из события, не из текста.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import QuestionsBlock from "../proto/QuestionsBlock.vue";
import type { Question, WorkflowEvent } from "../types";

const q = (over: Partial<Question> = {}): Question => ({
  id: "q1",
  task_id: "t1",
  execution_id: null,
  text: "Ревьюер вернул спеку: продолжаем круг или стоим?",
  context: null,
  options: [],
  blocking: true,
  status: "OPEN",
  answer: null,
  answered_by: null,
  answered_at: null,
  created_at: "2026-11-13T10:00:00Z",
  ...over,
});

const ev = (payload: unknown): WorkflowEvent => ({
  id: "e1", task_id: "t1", kind: "question.raised", actor: "system",
  payload, created_at: "2026-11-13T10:00:00Z",
});

describe("QuestionsBlock", () => {
  it("вопрос возврата ревью: два хода, каждый — слово человека", async () => {
    const w = mount(QuestionsBlock, {
      props: { questions: [q()], events: [ev({ question_id: "q1", gate: "review-return" })] },
    });
    expect(w.find(".atext").text()).toContain("продолжаем круг или стоим");
    const [go, stop] = w.findAll(".a");
    expect(go.text()).toBe("Продолжить круг");
    expect(stop.text()).toBe("Остановить");

    await go.trigger("click");
    expect(w.emitted("answer")![0]).toEqual([expect.objectContaining({ id: "q1" }), "продолжить"]);
    await stop.trigger("click");
    expect(w.emitted("answer")![1][1]).toBe("остановить");
  });

  it("обычный вопрос: свободный ответ, пустой не отправляется", async () => {
    const w = mount(QuestionsBlock, { props: { questions: [q({ text: "Как показывать отказ?" })], events: [] } });
    const btn = w.find(".afree .a");
    expect(btn.attributes("disabled")).toBeDefined();
    await w.find(".afree input").setValue("в строке шага");
    await btn.trigger("click");
    expect(w.emitted("answer")![0][1]).toBe("в строке шага");
  });

  it("отвеченные вопросы не висят в блоке", () => {
    const w = mount(QuestionsBlock, {
      props: { questions: [q({ status: "ANSWERED" })], events: [] },
    });
    expect(w.find(".ask").exists()).toBe(false);
  });

  it("гейт по чужому идентификатору не подставляет кнопки возврата", () => {
    const w = mount(QuestionsBlock, {
      props: { questions: [q()], events: [ev({ question_id: "other", gate: "review-return" })] },
    });
    expect(w.findAll(".abtns")).toHaveLength(0);
    expect(w.find(".afree").exists()).toBe(true);
  });
});
