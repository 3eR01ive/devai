// Тесты словаря и правил секции подписи (REQ-SIGN-02 AC4/AC5, REQ-SIGN-04,
// REQ-SIGN-11 AC4): словарь покрывает закрытые наборы и падает при
// пропущенном токене; отпускание раньше порога отменяет подпись; каждая
// ветка ревьювера даёт видимый текст; per-OQ выбор попадает в обоснование.
import { describe, expect, it } from "vitest";

import {
  ARTIFACT_STATUS_LABELS,
  CLOSED_TOKEN_SETS,
  DENIAL_LABELS,
  DIFF_PARAGRAPH_STATE_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  REVIEW_VERDICT_LABELS,
  SIGNATURE_KIND_LABELS,
  SIGNATURE_SOURCE_LABELS,
  labelFor,
} from "../signature_labels";
import {
  HOLD_MS,
  choicesValid,
  criteriaSummary,
  denialText,
  holdCompletes,
  mergeOqLines,
  oqRationaleLines,
  rationaleValid,
  reviewParagraph,
  reviewVerdict,
  signatureRationale,
  specParagraph,
} from "../signature_view";
import type { ReviewView, SpecView } from "../types";

const maps: Record<string, Record<string, string>> = {
  ARTIFACT_STATUS_LABELS,
  REVIEW_VERDICT_LABELS,
  PARAGRAPH_SOURCE_LABELS,
  REVIEW_PARAGRAPH_SOURCE_LABELS,
  DENIAL_LABELS,
  SIGNATURE_KIND_LABELS,
  SIGNATURE_SOURCE_LABELS,
  DIFF_PARAGRAPH_STATE_LABELS,
};

describe("signature_labels", () => {
  it("covers every closed token set the section prints", () => {
    for (const [name, tokens] of Object.entries(CLOSED_TOKEN_SETS)) {
      const map = maps[name];
      expect(map, `словарь ${name} отсутствует`).toBeDefined();
      for (const token of tokens) {
        expect(map[token], `${name}: нет слова для токена ${token}`).toBeTruthy();
      }
    }
  });

  it("does not invent words for unknown tokens", () => {
    const label = labelFor(ARTIFACT_STATUS_LABELS, "REVIEWED_SOMETHING");
    expect(label.raw).toBe(true);
    expect(label.text).toContain("REVIEWED_SOMETHING");
    expect(label.text).toContain("слово не задано");
  });
});

describe("hold-to-confirm", () => {
  it("releasing before the threshold cancels the act", () => {
    expect(holdCompletes(HOLD_MS - 1)).toBe(false);
    expect(holdCompletes(0)).toBe(false);
    expect(holdCompletes(HOLD_MS)).toBe(true);
  });
});

describe("signature rationale of the stored fact", () => {
  it("treats null and blank as «без обоснования», keeps a real text verbatim", () => {
    expect(signatureRationale(null)).toBeNull();
    expect(signatureRationale("")).toBeNull();
    expect(signatureRationale("   \n ")).toBeNull();
    expect(signatureRationale("принято как есть")).toBe("принято как есть");
    expect(signatureRationale("  строка с отступом\nвторая")).toBe(
      "  строка с отступом\nвторая",
    );
  });
});

const baseReview: ReviewView = {
  artifact: { seq: 2 } as ReviewView["artifact"],
  current: true,
  verdict: "approved",
  human_paragraph: "Абзац ревьювера.",
  paragraph_source: "review_report",
  note: null,
};

describe("review branches", () => {
  it("prints the reviewer's own paragraph", () => {
    const p = reviewParagraph(baseReview);
    expect(p.text).toBe("Абзац ревьювера.");
    expect(p.sourceNote).toContain("абзац отчёта");
  });

  it("prints summary when the report has no paragraph", () => {
    const p = reviewParagraph({
      ...baseReview,
      human_paragraph: "Резюме отчёта.",
      paragraph_source: "summary",
      note: "абзаца ревьювера в отчёте нет",
    });
    expect(p.text).toBe("Резюме отчёта.");
    expect(p.sourceNote).toContain("summary");
    expect(p.note).toContain("абзаца");
  });

  it("never leaves an empty block when the paragraph is absent", () => {
    const p = reviewParagraph({
      ...baseReview,
      human_paragraph: null,
      paragraph_source: "none",
      note: "в отчёте нет вердикта",
    });
    expect(p.text.length).toBeGreaterThan(0);
  });

  it("keeps the stored verdict token and adds a word", () => {
    expect(reviewVerdict(baseReview).token).toBe("approved");
    expect(reviewVerdict(baseReview).word).toBe("одобрено");
    const unknown = reviewVerdict({ ...baseReview, verdict: "custom_token" });
    expect(unknown.word).toContain("слово не задано");
  });
});

const baseSpec: SpecView = {
  artifact: { seq: 4 } as SpecView["artifact"],
  current: true,
  human_paragraph: "Строка один.\nСтрока два.",
  paragraph_source: "spec",
  requirements: [],
  requirements_note: null,
  open_questions: [],
  unavailable_note: null,
};

describe("spec paragraph and criteria", () => {
  it("prints the paragraph verbatim with its line breaks", () => {
    expect(specParagraph(baseSpec).text).toBe("Строка один.\nСтрока два.");
  });

  it("falls back to the business description label", () => {
    const p = specParagraph({
      ...baseSpec,
      human_paragraph: "бизнес-описание",
      paragraph_source: "business_description",
      unavailable_note: "спека до протокола",
    });
    expect(p.sourceNote).toBe("бизнес-описание задачи");
    expect(p.note).toBe("спека до протокола");
  });

  it("counts criteria without hiding them behind +N", () => {
    expect(criteriaSummary(0, 0)).toBe("критериев нет");
    expect(criteriaSummary(1, 0)).toContain("1 критерий");
    expect(criteriaSummary(3, 2)).toContain("3 критерия");
    expect(criteriaSummary(3, 2)).toContain("2 ограничения");
  });
});

describe("per-OQ choice and rationale", () => {
  it("defaults to «принято как есть» and writes the choice into the rationale", () => {
    const lines = oqRationaleLines([{ accept: true, answer: "" }]);
    expect(lines).toEqual(["Вопрос 1: принято как есть"]);
  });

  it("writes own answers; empty own answer is not signable", () => {
    const choices = [{ accept: false, answer: "свой путь" }];
    expect(oqRationaleLines(choices)).toEqual(["Вопрос 1: свой путь"]);
    expect(choicesValid(choices)).toBe(true);
    expect(choicesValid([{ accept: false, answer: "   " }])).toBe(false);
  });

  it("requires non-empty rationale", () => {
    expect(rationaleValid("")).toBe(false);
    expect(rationaleValid("   \n ")).toBe(false);
    expect(rationaleValid("подписываю")).toBe(true);
  });

  it("replaces OQ lines in the visible rationale, keeping manual text", () => {
    const merged = mergeOqLines("ручное пояснение\nВопрос 1: принято как есть", [
      { accept: false, answer: "иной ответ" },
    ]);
    expect(merged).toContain("ручное пояснение");
    expect(merged).toContain("Вопрос 1: иной ответ");
    expect(merged).not.toContain("принято как есть");
  });

  it("names the denial in words", () => {
    expect(denialText("no_spec")).toContain("спеки нет");
    expect(denialText(null)).toBeNull();
  });
});
