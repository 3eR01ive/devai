// Вставка кода: строки из ссылки подсвечены и после разворота файла целиком —
// иначе непонятно, ради чего её открывали.
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const sourceSlice = vi.fn();
vi.mock("../api", () => ({ api: { sourceSlice: (...a: any[]) => sourceSlice(...a) } }));

import CodeSlice from "../proto/CodeSlice.vue";

afterEach(() => {
  sourceSlice.mockReset();
  document.body.innerHTML = "";
});

const slice = (from: number, to: number, total: number) => ({
  path: "crates/x.rs",
  rev: "abc",
  from,
  to,
  total_lines: total,
  text: Array.from({ length: to - from + 1 }, (_, i) => `line ${from + i}`).join("\n"),
  candidates: [] as string[],
});

describe("CodeSlice", () => {
  it("подсвечивает строки из ссылки", async () => {
    sourceSlice.mockResolvedValue(slice(100, 110, 500));
    const w = mount(CodeSlice, {
      props: { taskId: "t", rev: "abc", path: "crates/x.rs", from: 100, to: 110 },
      attachTo: document.body,
    });
    await flushPromises();
    await nextTick();
    const pre = w.find("pre");
    expect(pre.attributes("data-line")).toBe("100-110");
    expect(pre.attributes("data-line-offset")).toBe("99");
    expect(document.querySelector(".line-highlight")).not.toBeNull();
  });

  it("после «развернуть весь файл» подсветка остаётся на тех же строках", async () => {
    sourceSlice.mockResolvedValueOnce(slice(100, 110, 500));
    const w = mount(CodeSlice, {
      props: { taskId: "t", rev: "abc", path: "crates/x.rs", from: 100, to: 110 },
      attachTo: document.body,
    });
    await flushPromises();
    await nextTick();

    sourceSlice.mockResolvedValueOnce(slice(1, 500, 500));
    const whole = w.findAll("button").find((b) => b.text().includes("весь файл"));
    expect(whole).toBeTruthy();
    await whole!.trigger("click");
    await flushPromises();
    await nextTick();

    const pre = w.find("pre");
    expect(pre.attributes("data-start")).toBe("1");
    expect(pre.attributes("data-line")).toBe("100-110");
    expect(pre.attributes("data-line-offset")).toBe("0");
    expect(document.querySelector(".line-highlight")).not.toBeNull();
  });

  it("при развороте держит подсвеченный кусок по центру окна кода", async () => {
    sourceSlice.mockResolvedValue(slice(1, 500, 500));
    const w = mount(CodeSlice, {
      props: { taskId: "t", rev: "abc", path: "crates/x.rs", from: 100, to: 110 },
      attachTo: document.body,
    });
    await flushPromises();
    await nextTick();

    const pre = w.find("pre").element as HTMLElement;
    const hl = document.querySelector(".line-highlight") as HTMLElement;
    expect(hl).not.toBeNull();
    // Теснота вёрстки: 520px окна, строка ссылки на 2000-й позиции, высотой 200.
    Object.defineProperty(hl, "offsetTop", { value: 2000, configurable: true });
    Object.defineProperty(hl, "offsetHeight", { value: 200, configurable: true });
    Object.defineProperty(pre, "clientHeight", { value: 520, configurable: true });

    (w.vm as any).centerOnLinked();
    expect(pre.scrollTop).toBe(1840);
  });

  it("отказ платформы печатается причиной, а не [object Object]", async () => {
    sourceSlice.mockRejectedValue({ kind: "no_source", message: "в коммите abc нет файла crates/x.rs" });
    const w = mount(CodeSlice, {
      props: { taskId: "t", rev: "abc", path: "crates/x.rs", from: 100, to: 110 },
      attachTo: document.body,
    });
    await flushPromises();
    await nextTick();
    const cap = w.find(".cap").text();
    expect(cap).not.toContain("[object Object]");
    expect(cap).toContain("нет файла");
  });

  it("неоднозначный путь: варианты файлов, выбор человека, без догадки", async () => {
    sourceSlice.mockResolvedValueOnce({
      path: "types.rs", rev: "abc", from: 27, to: 38, total_lines: 0, text: "",
      candidates: ["crates/devai-api/src/signature/types.rs", "crates/devai-core/src/types.rs"],
    });
    const w = mount(CodeSlice, {
      props: { taskId: "t", rev: "abc", path: "types.rs", from: 27, to: 38 },
      attachTo: document.body,
    });
    await flushPromises();
    await nextTick();

    const cands = w.findAll("button.cand");
    expect(cands.map((b) => b.text())).toEqual([
      "crates/devai-api/src/signature/types.rs",
      "crates/devai-core/src/types.rs",
    ]);
    expect(w.find("pre").exists()).toBe(false);

    sourceSlice.mockResolvedValueOnce(slice(27, 38, 700));
    await cands[1].trigger("click");
    await flushPromises();
    expect(sourceSlice).toHaveBeenLastCalledWith("t", "abc", "crates/devai-core/src/types.rs", 27, 38);
    expect(w.find("pre").exists()).toBe(true);
  });
});
