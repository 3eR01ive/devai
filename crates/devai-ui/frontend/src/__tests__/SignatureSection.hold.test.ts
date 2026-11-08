// Компонентный шов удержания (REQ-SIGN-04 AC3, rework R6): мутация уходит
// ровно один раз — когда удержание пересекло HOLD_MS; отпускание раньше
// порога не отправляет ничего. Машинерия живёт в useHoldToConfirm, поэтому
// тест проверяет именно emit('apply') из корня, а не только предикат
// holdCompletes.
import { mount } from "@vue/test-utils";
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
});
