// The QA gate's hold-actions at the component seam (REQ-QG-02/03/05,
// REQ-SAY-01/05): each button carries its consequence in words, the window
// adds only the non-empty rationale, the platform's refusal is shown next to
// the buttons, the return may go with drafted points alone, and a point of
// «не проверено» joins the draft with one click.
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import QualityGateActions from "../components/quality/QualityGateActions.vue";
import { ACCEPT_LABEL, RETURN_LABEL } from "../quality_gate_view";
import { HOLD_MS } from "../signature_view";
import type { QualityGateView } from "../types";

function gate(over: Partial<QualityGateView> = {}): QualityGateView {
  return {
    audit_running: false,
    current_report: "art-1",
    open_blockers: [],
    may_accept: true,
    accept_denied: null,
    may_return: true,
    return_denied: null,
    ...over,
  };
}

const draftItem = {
  key: "cl/fmt",
  item_id: "cl/fmt",
  summary: "cargo fmt дрейф",
  site: "a.rs:442",
  requirement: "cargo fmt дрейф",
};

let frames: FrameRequestCallback[];
let now: number;

beforeEach(() => {
  frames = [];
  now = 0;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    frames.push(cb);
    return frames.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {
    frames = [];
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function flushFrames() {
  const pending = frames;
  frames = [];
  for (const frame of pending) frame(0);
}

function mountActions(over: Partial<InstanceType<typeof QualityGateActions>["$props"]> = {}) {
  return mount(QualityGateActions, {
    props: {
      gate: gate(),
      draft: [],
      busy: false,
      error: null,
      draftNote: "все замечания ниже уже здесь — сними те, что не надо возвращать.",
      unchecked: [],
      ...over,
    },
  });
}

describe("QualityGateActions", () => {
  it("signs both buttons with their consequence, never with an internal state", () => {
    const w = mountActions();
    const accept = w.find("button.hold.approve").text();
    const rework = w.find("button.hold.rework").text();
    expect(accept).toContain(ACCEPT_LABEL);
    expect(rework).toContain(RETURN_LABEL);
    for (const label of [accept, rework]) {
      expect(label).not.toMatch(/ACCEPTED|REWORK|INCONSISTENT|PASS|FAIL/);
    }
    // The hold mechanic is explained beside the pair, not inside the label.
    expect(w.text()).toContain("удерживай");
    w.unmount();
  });

  it("keeps both actions disabled and explains why while the rationale is empty", async () => {
    const w = mountActions();
    const accept = w.find("button.hold.approve");
    const rework = w.find("button.hold.rework");
    expect(accept.attributes("disabled")).toBeDefined();
    expect(rework.attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("без обоснования принять нельзя");
    expect(w.text()).toContain("без обоснования или пунктов возврат недоступен");

    await w.find("textarea").setValue("принято человеком");
    expect(w.find("button.hold.approve").attributes("disabled")).toBeUndefined();
    // The return still needs its own rationale/draft: accept text alone is a
    // rationale, so it is enabled too — the platform's may_return is true.
    expect(w.find("button.hold.rework").attributes("disabled")).toBeUndefined();
    w.unmount();
  });

  it("sends one accept after the hold crosses the threshold, and nothing on an early release", async () => {
    const w = mountActions();
    await w.find("textarea").setValue("принято человеком");
    await w.find("button.hold.approve").trigger("pointerdown");
    now = HOLD_MS - 1;
    await w.find("button.hold.approve").trigger("pointerup");
    flushFrames();
    expect(w.emitted("apply")).toBeUndefined();

    now = 0;
    await w.find("button.hold.approve").trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    const emits = w.emitted("apply");
    expect(emits).toHaveLength(1);
    expect(emits?.[0]).toEqual(["accept", "принято человеком"]);
    w.unmount();
  });

  it("lets the return go with drafted points alone and sends the composed text", async () => {
    const w = mountActions({ draft: [draftItem] });
    // No rationale typed: the drafted requirement itself is the rationale.
    expect(w.find("button.hold.rework").attributes("disabled")).toBeUndefined();
    await w.find("button.hold.rework").trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    const emits = w.emitted("apply");
    expect(emits).toHaveLength(1);
    const [kind, text] = emits?.[0] as [string, string];
    expect(kind).toBe("return");
    expect(text).toContain("cargo fmt дрейф");
    expect(text).toContain("где: a.rs:442");
    w.unmount();
  });

  it("shows the filled draft as the points that will go back, one text field each", async () => {
    const w = mountActions({ draft: [draftItem] });
    expect(w.text()).toContain("1 пункт(ов) уйдёт вместе с возвратом");
    expect(w.text()).toContain("сними те, что не надо возвращать");
    const input = w.find(".draft-item input");
    expect((input.element as HTMLInputElement).value).toBe("cargo fmt дрейф");
    await input.setValue("поправить форматирование");
    const emitted = w.emitted("update:draft")?.[0]?.[0] as { requirement: string }[];
    expect(emitted[0].requirement).toBe("поправить форматирование");
    // A row can be taken out with one click: the operator removes, not adds.
    await w.find(".draft-item button.remove").trigger("click");
    expect(w.emitted("update:draft")).toHaveLength(2);
    w.unmount();
  });

  it("adds a «не проверено» point to the draft with one click", async () => {
    const w = mountActions({
      unchecked: [
        {
          id: "brk/x",
          what: "проверка не прошла",
          site: "",
          note: "нужен повторный прогон",
          missing: "probe could not run",
          line: "brk/x · балл —",
        },
      ],
    });
    expect(w.text()).toContain("не проверено: проверка не прошла");
    await w.find(".unchecked-add button").trigger("click");
    const emitted = w.emitted("update:draft")?.[0]?.[0] as { item_id: string; site: string }[];
    expect(emitted).toHaveLength(1);
    expect(emitted[0].item_id).toBe("brk/x");
    expect(emitted[0].site).toBe("");
    w.unmount();
  });

  it("shows the platform refusal next to the buttons and never hides the other act", async () => {
    const w = mountActions({
      gate: gate({
        may_accept: false,
        accept_denied: { code: "no_current_report", text: "нет текущего отчёта качества — принять нечего" },
      }),
    });
    const accept = w.find("button.hold.approve");
    expect(accept.attributes("disabled")).toBeDefined();
    expect(w.text()).toContain("принять нельзя: нет текущего отчёта качества");
    expect(w.find("button.hold.rework").exists()).toBe(true);
    // The return is the second, independent act: once ITS rationale is there,
    // the accept refusal does not disable it.
    await w.find("textarea").setValue("вернуть в доработку");
    expect(w.find("button.hold.rework").attributes("disabled")).toBeUndefined();
    w.unmount();
  });

  it("keeps the typed error beside the buttons and preserves the typed rationale", async () => {
    const w = mountActions({
      error: { code: "guard", message: "acceptance requires a CURRENT QUALITY_REPORT artifact" },
    });
    await w.find("textarea").setValue("мой текст");
    expect(w.find(".gate-error").text()).toContain("guard");
    expect(w.find(".gate-error").text()).toContain("CURRENT QUALITY_REPORT");
    expect((w.find("textarea").element as HTMLTextAreaElement).value).toBe("мой текст");
    w.unmount();
  });
});
