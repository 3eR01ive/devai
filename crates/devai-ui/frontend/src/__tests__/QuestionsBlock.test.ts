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

  it("протокол раскладывается по пунктам, и ответ даётся на КАЖДЫЙ вопрос", async () => {
    const w = mount(QuestionsBlock, {
      props: {
        questions: [q()],
        events: [],
        protocol: {
          verdict: "QUESTIONS",
          human_paragraph: "не хватает границ и критерия",
          checklist: [
            { item: "user", ok: true, note: null },
            { item: "boundaries", ok: false, note: "не указаны" },
          ],
          questions: ["Где границы задачи?", "Что считается сделанным?"],
        },
      },
    });
    // чек-лист — строки, а не текстом
    expect(w.find(".pverdict").text()).toContain("есть вопросы");
    const rows = w.findAll(".pcheck");
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("пользователь");
    expect(rows[1].classes()).toContain("bad");

    // по полю на каждый вопрос; пока пусто — отправлять нечего
    const areas = w.findAll(".pq textarea");
    expect(areas).toHaveLength(2);
    expect(w.find(".abtns .a.go").attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("0 из 2 с ответом");

    await areas[0].setValue("границы: только наш дом");
    await areas[1].setValue("сделанным считается запуск без ошибок");
    expect(w.text()).toContain("2 из 2 с ответом");
    await w.find(".abtns .a.go").trigger("click");

    const sent = (w.emitted("answer")![0] as [unknown, string])[1];
    expect(sent).toContain("1) Где границы задачи?");
    expect(sent).toContain("Ответ: границы: только наш дом");
    expect(sent).toContain("2) Что считается сделанным?");
    expect(sent).toContain("Ответ: сделанным считается запуск без ошибок");

    // склеенный текст вопроса не дублируется, когда протокол разложен
    expect(w.find(".atext").exists()).toBe(false);
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
