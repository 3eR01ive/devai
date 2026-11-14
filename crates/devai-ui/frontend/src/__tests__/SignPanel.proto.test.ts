// Гейт подписи: вопрос отделён от предложенного ответа, ответ правится,
// решения уходят в rationale подписи, акта два и их доступность берётся
// из платформенного чтения, а не из догадки окна.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import SignPanel from "../proto/SignPanel.vue";
import { buildRationale, oqParts } from "../proto/signText";
import type { SignatureView, SpecView } from "../types";

function spec(overrides: Partial<SpecView> = {}): SpecView {
  return {
    artifact: {
      id: "a1", task_id: "t1", kind: "REQUIREMENTS_SPEC", name: "spec",
      seq: 4, category: "spec", status: "CURRENT", sha256: "s", size: 1,
      media_type: "application/json", contract: null, created_by: "analyst",
      created_at: "2026-11-12T10:00:00Z",
    } as any,
    current: true,
    human_paragraph: "Спека про остановку.",
    paragraph_source: "spec",
    requirements: [
      {
        id: "REQ-1", statement: "платформа стоит", acceptance_criteria: ["а", "б"],
        anchors: [], kind: "functional", notes: null,
      } as any,
    ],
    requirements_note: null,
    open_questions: [
      "OQ-1 блокирующий: останавливать сразу или после круга? Предлагаю: останавливать сразу.",
      "OQ-2: как показывать отказ?",
    ],
    unavailable_note: null,
    ...overrides,
  };
}

function view(overrides: Partial<SignatureView> = {}): SignatureView {
  return {
    task_id: "t1",
    state: "APPROVAL",
    snapshot_at: "2026-11-12T10:00:00Z",
    may_approve: true,
    may_approve_denied: null,
    may_reject: true,
    may_reject_denied: null,
    spec: spec(),
    review: null,
    review_note: null,
    previous: null,
    previous_note: null,
    signature: null,
    ...overrides,
  };
}

describe("signText", () => {
  it("делит вопрос и предложение", () => {
    const p = oqParts("вопрос? Предлагаю: ответ.");
    expect(p.question).toBe("вопрос?");
    expect(p.proposal).toBe("ответ.");
  });

  it("без предложения вопрос остаётся целым", () => {
    const p = oqParts("вопрос без предложения");
    expect(p.question).toBe("вопрос без предложения");
    expect(p.proposal).toBe("");
  });

  it("решения собираются по строкам, пустые не попадают", () => {
    const list = [
      { k: 0, question: "a", proposal: "" },
      { k: 1, question: "b", proposal: "" },
    ];
    expect(buildRationale(list, { 0: "так", 1: "  " }, "и ещё")).toBe("Вопрос 1: так\nи ещё");
    expect(buildRationale(list, {}, "")).toBeNull();
  });
});

describe("SignPanel", () => {
  it("вопрос и решение разведены, предложение подставлено в редактор", async () => {
    const w = mount(SignPanel, { props: { signature: view() } });
    const cards = w.findAll(".oqcard");
    expect(cards).toHaveLength(2);
    // вопрос — в верхней части карточки, решение — в нижней
    expect(cards[0].find(".qq").text()).toContain("останавливать сразу или после круга");
    expect(cards[0].find(".qq").text()).not.toContain("Предлагаю");
    const ta0 = cards[0].find(".qa textarea").element as HTMLTextAreaElement;
    const ta1 = cards[1].find(".qa textarea").element as HTMLTextAreaElement;
    expect(ta1.value).toBe("");
    expect(ta0.value).toBe("останавливать сразу.");
    // вопрос и решение помечены разными метками, а не сливаются в текст
    expect(cards[0].find(".qq .pill").text()).toBe("вопрос 1");
    expect(cards[0].find(".qa .pill").text()).toBe("твоё решение");
    expect(cards[0].find(".qa .src").text()).toContain("предложено аналитиком");
    expect(cards[1].find(".qa .src").text()).toContain("решения нет");
  });

  it("правка уходит в rationale подписи", async () => {
    const w = mount(SignPanel, { props: { signature: view() } });
    const ta = w.findAll(".qa textarea")[1];
    await ta.setValue("показывать отказ в строке шага");
    await w.find(".signnote input").setValue("подписываю с правкой");
    await w.find(".act.go").trigger("click");

    const [approve, rationale] = w.emitted("sign")![0] as [boolean, string];
    expect(approve).toBe(true);
    expect(rationale).toContain("Вопрос 1: останавливать сразу.");
    expect(rationale).toContain("Вопрос 2: показывать отказ в строке шага");
    expect(rationale).toContain("подписываю с правкой");
  });

  it("отклонить — второй акт, доступность из чтения платформы", async () => {
    const w = mount(SignPanel, {
      props: { signature: view({ may_approve: false, may_approve_denied: "открытые блокеры" }) },
    });
    expect(w.find(".act.go").attributes("disabled")).toBeDefined();
    expect(w.find(".why").text()).toContain("открытые блокеры");
    await w.find(".act.no").trigger("click");
    expect(w.emitted("sign")![0][0]).toBe(false);
  });

  it("без спеки подписывать нечего", () => {
    const w = mount(SignPanel, { props: { signature: view({ spec: null }) } });
    expect(w.text()).toContain("подписывать нечего");
    expect(w.findAll(".act")).toHaveLength(0);
  });
});
