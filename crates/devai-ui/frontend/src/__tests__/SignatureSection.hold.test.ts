// Компонентный шов удержания (REQ-SIGN-04 AC3, rework R6): мутация уходит
// ровно один раз — когда удержание пересекло HOLD_MS; отпускание раньше
// порога не отправляет ничего. Машинерия живёт в useHoldToConfirm, поэтому
// тест проверяет именно emit('apply') из корня, а не только предикат
// holdCompletes.
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import SignatureSection from "../components/SignatureSection.vue";
import type { SignatureView, SpecView } from "../types";
import { HOLD_MS } from "../signature_view";

function specView(): SpecView {
  return {
    artifact: { seq: 4, status: "CURRENT" } as SpecView["artifact"],
    current: true,
    human_paragraph: "Абзац.",
    paragraph_source: "spec",
    requirements: [],
    requirements_note: null,
    open_questions: [],
    unavailable_note: null,
  };
}

function signatureView(): SignatureView {
  return {
    task_id: "t1",
    state: "APPROVAL",
    snapshot_at: "2026-11-08T10:00:00Z",
    may_approve: true,
    may_approve_denied: null,
    may_reject: true,
    may_reject_denied: null,
    spec: specView(),
    review: null,
    review_note: "отчёта ревьювера нет",
    previous: null,
    previous_note: "прошлой версии нет",
    signature: null,
  };
}

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

/** Pointer-событие с настоящими координатами: смахивание проверяется по
 *  сдвигу от точки нажатия, а не по факту ухода с кнопки. */
async function fire(el: Element, type: string, x: number, y: number) {
  el.dispatchEvent(
    new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y }),
  );
  await nextTick();
}

describe("hold-to-confirm at the component seam", () => {
  it("emits one apply with the visible rationale when the hold crosses the threshold", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("textarea").setValue("подписываю");
    await w.find("button.hold.approve").trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    const emits = w.emitted("apply");
    expect(emits).toHaveLength(1);
    expect(emits?.[0]).toEqual(["approve", "подписываю"]);
    w.unmount();
  });

  it("a release before the threshold sends nothing", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("textarea").setValue("подписываю");
    await w.find("button.hold.reject").trigger("pointerdown");
    now = HOLD_MS - 1;
    await w.find("button.hold.reject").trigger("pointerup");
    flushFrames();
    expect(w.emitted("apply")).toBeUndefined();
    w.unmount();
  });

  it("an empty rationale blocks the hold entirely", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("button.hold.approve").trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    expect(w.emitted("apply")).toBeUndefined();
    w.unmount();
  });

  // REQ-RESP-06: палец смахивает — удержание отменяется ДАЖЕ внутри кнопки;
  // прогресс сброшен, мутации нет.
  it("a swipe past 40px cancels the hold and leaves no progress", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("textarea").setValue("подписываю");
    const btn = w.find("button.hold.approve");
    await fire(btn.element, "pointerdown", 10, 10);
    await fire(btn.element, "pointermove", 60, 10);
    now = HOLD_MS + 10;
    flushFrames();
    expect(w.emitted("apply")).toBeUndefined();
    expect(w.find("button.hold.approve").classes()).not.toContain("holding");
    w.unmount();
  });

  // Дрожание пальца в пределах порога удержание не срывает.
  it("a small tremor within 40px does not cancel the hold", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("textarea").setValue("подписываю");
    const btn = w.find("button.hold.approve");
    await fire(btn.element, "pointerdown", 10, 10);
    await fire(btn.element, "pointermove", 35, 10);
    now = HOLD_MS + 10;
    flushFrames();
    expect(w.emitted("apply")).toEqual([["approve", "подписываю"]]);
    w.unmount();
  });

  // Отпускание вне кнопки (pointerleave) мутации не даёт.
  it("releasing outside the button cancels before the threshold", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("textarea").setValue("подписываю");
    const btn = w.find("button.hold.approve");
    await fire(btn.element, "pointerdown", 10, 10);
    await fire(btn.element, "pointerleave", 10, 10);
    now = HOLD_MS + 10;
    flushFrames();
    expect(w.emitted("apply")).toBeUndefined();
    w.unmount();
  });

  // REQ-RESP-06 AC5: одиночное и двойное касание не применяют подпись —
  // обработчика click нет, а короткие нажатия не пересекают порог.
  it("a single and a double tap apply nothing", async () => {
    const w = mount(SignatureSection, { props: { signature: signatureView(), busy: false } });
    await w.find("textarea").setValue("подписываю");
    const btn = w.find("button.hold.approve");
    for (let i = 0; i < 2; i++) {
      await fire(btn.element, "pointerdown", 10, 10);
      now += 120;
      flushFrames();
      await fire(btn.element, "pointerup", 10, 10);
    }
    now = HOLD_MS + 10;
    flushFrames();
    expect(w.emitted("apply")).toBeUndefined();
    w.unmount();
  });
});
