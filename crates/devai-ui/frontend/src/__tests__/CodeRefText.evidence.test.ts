// Регресс: свидетельство приёмки с ссылками на код не должно терять слова.
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { describe, expect, it, vi } from "vitest";

vi.mock("../api", () => ({ api: { sourceSlice: vi.fn(async () => ({ path: "x", rev: "r", from: 1, to: 1, total_lines: 1, text: "x" })) } }));
import CodeRefText from "../proto/CodeRefText.vue";

const TEXT = "Основная модель (граф, позиция, гейты, петля, имена в UI) соответствует REQ-RG-01..06/REQ-NM-01, но два пункта принятой спеки не закрыты. (1) AC4 REQ-NM-01 требует «подпись есть во всех ответах с артефактом»: SpecView и ReviewView несут artifact без display (signature/types.rs:15-24, 29-36), и карточка подписи сама склеивает заголовок из seq (SignatureSection.vue:66-70) — заголовок спеки на утверждении остаётся собранным интерфейсом. (2) Общее правило «id никогда не занимает место заголовка, если имя есть» не применено на CLI-списках: task list печатает short id первым, имя последним (commands.rs:646-657), task active — short id первым и роль как slug@version (commands.rs:3098-3102), хотя ActiveRow уже несёт task_display/role_display (lib.rs:767-769), а UI их использует (LiveView.vue:66-70). Границы и семантика целы, поэтому 0.4, а не 0.1.";

describe("CodeRefText / свидетельство", () => {
  it("не превращает слова в [object Object]", async () => {
    const w = mount(CodeRefText, { props: { text: TEXT, taskId: "t", rev: "abc" } });
    await nextTick();
    const out = w.text();
    expect(out).not.toContain("[object Object]");
    expect(out).toContain("подпись есть во всех ответах");
  });
});
