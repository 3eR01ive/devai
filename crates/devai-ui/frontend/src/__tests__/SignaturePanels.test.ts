// Rework R2/R3: панели результата подписи и диффа спеки раньше не
// монтировались ни одним тестом, а пустое обоснование (Some("")) печатало
// пустой абзац под меткой «обоснование:». Здесь панели исполняются как в
// окне (Vue-рендерер + @vue/test-utils), и проверяются: маркер «без
// обоснования» для null и пустой строки, содержимое факта, строки диффа и
// ветка «отчёт не CURRENT».
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import SignatureResultPanel from "../components/SignatureResultPanel.vue";
import SpecDiffPanel from "../components/SpecDiffPanel.vue";
import SignatureReviewer from "../components/signature/SignatureReviewer.vue";
import type { PreviousView, ReviewView, SignatureFact } from "../types";
import { artifact } from "./fixtures";

function fact(overrides: Partial<SignatureFact> = {}): SignatureFact {
  return {
    kind: "approval",
    actor: "guerrero",
    at: "2026-11-08T10:00:00Z",
    rationale: "подписываю",
    spec_seq: 4,
    source: "decision",
    ...overrides,
  };
}

function mountResult(value: SignatureFact) {
  return mount(SignatureResultPanel, { props: { fact: value } });
}

describe("SignatureResultPanel", () => {
  it("prints who, when, the rationale and the spec version", () => {
    const w = mountResult(fact());
    const text = w.text();
    expect(text).toContain("Подпись спеки · результат");
    expect(text).toContain("утверждение");
    expect(text).toContain("guerrero");
    expect(text).toContain("подписываю");
    expect(text).toContain("4");
    expect(text).not.toContain("без обоснования");
    w.unmount();
  });

  it("prints «без обоснования» when the stored rationale is null", () => {
    const w = mountResult(fact({ rationale: null, spec_seq: null }));
    const text = w.text();
    expect(text).toContain("без обоснования");
    expect(text).toContain("версия в записи не сохранена");
    w.unmount();
  });

  it("treats a stored EMPTY rationale (Some(\"\")) as «без обоснования»", () => {
    const w = mountResult(fact({ rationale: "" }));
    const paragraphs = w.findAll("p.rationale");
    expect(paragraphs).toHaveLength(1);
    expect(paragraphs[0].text()).toBe("без обоснования");
    w.unmount();
  });

  it("treats a whitespace-only rationale as «без обоснования»", () => {
    const w = mountResult(fact({ rationale: "   \n " }));
    expect(w.text()).toContain("без обоснования");
    w.unmount();
  });
});

function previous(overrides: Partial<PreviousView["diff"]> = {}): PreviousView {
  return {
    artifact: artifact("spec-prev", 3),
    diff: {
      paragraph: [
        { mark: " ", label: "абзац", text: "общая строка" },
        { mark: "-", label: "абзац", text: "старая строка" },
        { mark: "+", label: "абзац", text: "новая строка" },
      ],
      requirements: [{ mark: "+", label: "REQ-2", text: "Новое требование" }],
      refusal: null,
      note: null,
      ...overrides,
    },
  };
}

describe("SpecDiffPanel", () => {
  it("renders the human +/− lines of both versions, not raw JSON", () => {
    const w = mount(SpecDiffPanel, { props: { previous: previous() } });
    const text = w.text();
    expect(text).toContain("Дифф спеки");
    expect(text).toContain("общая строка");
    expect(text).toContain("старая строка");
    expect(text).toContain("новая строка");
    expect(text).toContain("REQ-2");
    expect(w.findAll(".dline.del")).toHaveLength(1);
    expect(w.findAll(".dline.add")).toHaveLength(2);
    expect(w.findAll(".dline.ctx")).toHaveLength(1);
    expect(text).not.toContain("{");
    w.unmount();
  });

  it("shows the refusal note instead of an invented diff", () => {
    const w = mount(SpecDiffPanel, {
      props: {
        previous: previous({
          paragraph: [],
          requirements: [],
          note: "сравнить не удалось: прошлая версия не читается",
        }),
      },
    });
    const text = w.text();
    expect(text).toContain("сравнить не удалось");
    expect(text).toContain("прошлая версия не читается");
    expect(w.findAll(".dline")).toHaveLength(0);
    w.unmount();
  });
});

function review(overrides: Partial<ReviewView> = {}): ReviewView {
  return {
    artifact: artifact("review-1", 3),
    current: true,
    verdict: "approved",
    human_paragraph: "Одобряю.",
    paragraph_source: "review_report",
    note: null,
    ...overrides,
  };
}

describe("SignatureReviewer not-current branch", () => {
  it("marks a report that is not CURRENT and keeps its own seq", () => {
    const w = mount(SignatureReviewer, {
      props: { review: review({ current: false }), note: null },
    });
    const text = w.text();
    expect(text).toContain("версия 3");
    expect(text).toContain("(не CURRENT)");
    w.unmount();
  });

  it("does not print the mark for a CURRENT report", () => {
    const w = mount(SignatureReviewer, {
      props: { review: review(), note: null },
    });
    expect(w.text()).not.toContain("(не CURRENT)");
    w.unmount();
  });
});
