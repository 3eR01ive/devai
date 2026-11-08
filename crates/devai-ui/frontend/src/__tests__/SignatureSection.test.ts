// Компонентные тесты секции подписи (rework R2, REQ-SIGN-01 AC4 /
// REQ-SIGN-11): блок «Отчёт ревьювера» — часть экрана подписи ВСЕГДА, в том
// числе когда спеки нет вовсе (spec=null). Раньше этот блок жил внутри
// ветки `v-else` спеки и при spec=null исчезал целиком.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import SignatureSection from "../components/SignatureSection.vue";
import type { ReviewView, SignatureView, SpecView } from "../types";
import { artifact } from "./fixtures";

function specView(overrides: Partial<SpecView> = {}): SpecView {
  return {
    artifact: artifact("spec-1", 4),
    current: true,
    human_paragraph: "Абзац спеки.",
    paragraph_source: "spec",
    requirements: [],
    requirements_note: null,
    open_questions: [],
    unavailable_note: null,
    ...overrides,
  };
}

function reviewView(overrides: Partial<ReviewView> = {}): ReviewView {
  return {
    artifact: artifact("review-1", 3),
    current: true,
    verdict: "approved",
    human_paragraph: "Ревьювер одобряет.",
    paragraph_source: "review_report",
    note: null,
    ...overrides,
  };
}

function signatureView(overrides: Partial<SignatureView> = {}): SignatureView {
  return {
    task_id: "t1",
    state: "APPROVAL",
    snapshot_at: "2026-11-08T10:00:00Z",
    may_approve: false,
    may_approve_denied: "no_spec",
    may_reject: true,
    may_reject_denied: null,
    spec: null,
    review: null,
    review_note: "отчёта ревьювера нет",
    previous: null,
    previous_note: "прошлой версии нет",
    signature: null,
    ...overrides,
  };
}

function mountWith(view: SignatureView) {
  return mount(SignatureSection, { props: { signature: view, busy: false } });
}

describe("SignatureSection without a spec", () => {
  it("still shows the reviewer block and says honestly that no report exists", () => {
    const w = mountWith(signatureView());
    const text = w.text();
    expect(text).toContain("Спека на утверждение");
    expect(text).toContain("утверждать нечего");
    // R2: the reviewer block is NOT inside the spec branch.
    expect(text).toContain("Отчёт ревьювера");
    expect(text).toContain("отчёта ревьювера нет");
    w.unmount();
  });

  it("prints the stored verdict and paragraph when a report exists but the spec does not", () => {
    const w = mountWith(
      signatureView({
        may_approve_denied: "no_spec",
        review: reviewView(),
        review_note: null,
      }),
    );
    const text = w.text();
    expect(text).toContain("Отчёт ревьювера");
    expect(text).toContain("approved");
    expect(text).toContain("Ревьювер одобряет.");
    w.unmount();
  });

  it("offers the reject action without a spec and names the approve refusal in words", () => {
    const w = mountWith(signatureView());
    // No approve BUTTON (the right is false); the refusal is named in words.
    expect(w.text()).not.toContain("Утвердить — удерживайте");
    expect(w.text()).toContain("«Утвердить» недоступно");
    expect(w.text()).toContain("спеки нет");
    expect(w.text()).toContain("Отклонить");
    w.unmount();
  });
});

describe("SignatureSection with a spec", () => {
  it("shows the paragraph, the requirements and the reviewer block together", () => {
    const w = mountWith(
      signatureView({
        may_approve: true,
        may_approve_denied: null,
        spec: specView({
          requirements: [
            {
              id: "REQ-1",
              statement: "Сделать X",
              acceptance_criteria: ["AC1"],
              constraints: [],
            },
          ],
        }),
        review: reviewView(),
        review_note: null,
      }),
    );
    const text = w.text();
    expect(text).toContain("Абзац спеки.");
    expect(text).toContain("REQ-1");
    expect(text).toContain("Отчёт ревьювера");
    expect(text).toContain("approved");
    w.unmount();
  });

  // Rework R4: the render branch of a spec that is NOT CURRENT has its own
  // component test — the notice must be visible (not only "not decided by
  // may_approve").
  it("marks a spec that is not CURRENT, with its own version, and still offers reject", () => {
    const w = mountWith(
      signatureView({
        may_approve: false,
        may_approve_denied: "spec_not_current",
        may_reject: true,
        spec: specView({ current: false }),
      }),
    );
    const text = w.text();
    expect(text).toContain("не CURRENT");
    expect(text).toContain("версия 4");
    expect(text).toContain("Отклонить");
    expect(text).not.toContain("Утвердить — удерживайте");
    w.unmount();
  });

  // Rework R6 (spec-fidelity): the DEFAULT position of every document
  // question («принять как есть») is part of what the operator signs, so it
  // must already sit in the visible rationale field — not appear only after
  // the first edit.
  it("writes the default «принято как есть» of every question into the rationale field", () => {
    const w = mountWith(
      signatureView({
        may_approve: true,
        may_approve_denied: null,
        spec: specView({ open_questions: ["OQ-1: вопрос", "OQ-2: второй"] }),
        review: reviewView(),
        review_note: null,
      }),
    );
    const field = w.find("textarea");
    expect(field.exists()).toBe(true);
    const value = (field.element as HTMLTextAreaElement).value;
    expect(value).toContain("Вопрос 1: принято как есть");
    expect(value).toContain("Вопрос 2: принято как есть");
    w.unmount();
  });
});
