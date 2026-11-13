// Ссылки на код в тексте: чип, вставка СВЁРНУТА до клика, клик открывает
// CodeSlice ровно для этой ссылки; у ссылки без коммита — честная строка
// вместо пустоты. Один компонент на все места, где в тексте есть «путь:строки».
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

const sourceSlice = vi.fn();
vi.mock("../api", () => ({
  api: {
    sourceSlice: (...a: any[]) => sourceSlice(...a),
  },
}));

import CodeRefText from "../proto/CodeRefText.vue";

afterEach(() => {
  sourceSlice.mockReset();
});

describe("CodeRefText", () => {
  it("рисует ссылку чипом и НЕ грузит код, пока не кликнули", async () => {
    const w = mount(CodeRefText, {
      props: {
        text: "проверено (crates/devai-core/src/workflow.rs:1784-1794), остальное в порядке",
        taskId: "01a1b5e1-dea0-7495-a293-d486e3678144",
        rev: "abc123",
      },
    });
    await nextTick();
    expect(w.findAll("button")).toHaveLength(1);
    expect(w.text()).toContain("crates/devai-core/src/workflow.rs:1784–1794");
    expect(w.findComponent({ name: "CodeSlice" }).exists()).toBe(false);
    expect(sourceSlice).not.toHaveBeenCalled();
  });

  it("клик по чипу открывает вставку именно этой ссылки", async () => {
    sourceSlice.mockResolvedValue({
      path: "crates/devai-core/src/workflow.rs",
      rev: "abc123",
      from: 1784,
      to: 1794,
      total_lines: 2043,
      text: "1784 line",
    });
    const w = mount(CodeRefText, {
      props: {
        text: "см. crates/devai-core/src/workflow.rs:1784-1794",
        taskId: "t",
        rev: "abc123",
      },
    });
    await w.find("button").trigger("click");
    await nextTick();
    expect(sourceSlice).toHaveBeenCalledTimes(1);
    expect(sourceSlice).toHaveBeenCalledWith("t", "abc123", "crates/devai-core/src/workflow.rs", 1784, 1794);
    expect(w.findComponent({ name: "CodeSlice" }).exists()).toBe(true);
  });

  it("без коммита вставки нет — вместо неё сказано почему", async () => {
    const w = mount(CodeRefText, {
      props: { text: "см. crates/devai-api/src/lib.rs:10-12", taskId: "t", rev: "" },
    });
    await w.find("button").trigger("click");
    await nextTick();
    expect(w.findComponent({ name: "CodeSlice" }).exists()).toBe(false);
    expect(w.text()).toContain("код недоступен");
  });

  it("текст без ссылок остаётся текстом, кнопок нет", async () => {
    const w = mount(CodeRefText, {
      props: { text: "замечаний нет", taskId: "t", rev: "abc123" },
    });
    await nextTick();
    expect(w.findAll("button")).toHaveLength(0);
  });
});
