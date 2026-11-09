// The QA report panel at the component seam (rework R5): the template wiring
// the pure modules cannot prove — counters before any expansion, the native
// <details> rows with their evidence/site layer, the saved-group filter, and
// addPoint → draft → the hold-action's rationale. IPC is not involved: the
// panel renders the props it is given.
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import QualityGateActions from "../components/quality/QualityGateActions.vue";
import QualityGatePanel from "../components/quality/QualityGatePanel.vue";
import { ALL_GROUPS } from "../report_view";
import { HOLD_MS } from "../signature_view";
import type { QualityGateView, QualityReportView } from "../types";

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

function report(): QualityReportView {
  const one = (id: string, summary: string, site: string, evidence?: string[]) => ({
    id, kind: "scored", summary, site, evidence,
    line: `${id} · балл 0.900`, score: 0.9, weight: 0.1, importance: 0.05,
  });
  return {
    task: "t1",
    artifact: "01a1b2aa-1111-700a-aaaa-111111111111",
    seq: 1,
    contract: "quality-report@5",
    created_at: "2026-11-09T10:00:00Z",
    execution: null,
    status: "CURRENT",
    source: "human_form",
    report_text: null,
    freshness: null,
    report: {
      human_view: {
        version: 1,
        lang: "ru",
        headline: {
          verdict: "НЕ ПРОШЁЛ", machine_verdict: "FAIL", score: 0.82, theta: 0.9,
          numbers: "S=0.820/1.00, порог 0.900", basis: "красные линии: cl/fmt",
          counts: { blocks: 2, nuances: 1, anchors: 0, unchecked: 1 },
        },
        groups: [
          {
            key: "blocks", title: "БЬЁТ ПО ВЕРДИКТУ", lead: "решается тобой",
            items: [
              one("cl/fmt", "cargo fmt дрейф", "a.rs:442", ["probe: fmt отстаёт"]),
              { ...one("cl/nosite", "без сайта", ""), score: 0.5 },
            ],
          },
          {
            key: "nuances", title: "НЮАНСЫ", lead: "",
            items: [one("n/one", "мелочь", "b.rs:1")],
          },
        ],
        blocked_weight: 0.1,
        max_blocked_weight: 0.2,
      },
    },
  };
}

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

function mountPanel(g: QualityGateView = gate()) {
  return mount(QualityGatePanel, {
    props: { report: report(), reportError: null, gate: g, busy: false, error: null },
  });
}

function flushFrames() {
  const pending = frames;
  frames = [];
  for (const frame of pending) frame(0);
}

describe("QualityGatePanel", () => {
  it("prints the verdict, numbers and every saved group's counter without expanding", () => {
    const w = mountPanel();
    const text = w.text();
    expect(text).toContain("S=0.820/1.00, порог 0.900");
    expect(text).toContain("бьёт по вердикту: 2");
    expect(text).toContain("нюансы: 1");
    // The rows are the stored lines; two blocks rows expand, anchors have none.
    expect(w.findAll("details.qr-item")).toHaveLength(3);
    expect(text).toContain("БЬЁТ ПО ВЕРДИКТУ (2)");
    expect(text).toContain("НЮАНСЫ (1)");
    w.unmount();
  });

  it("keeps the stored evidence and names an absent site inside the expandable row", () => {
    const w = mountPanel();
    const rows = w.findAll("details.qr-item");
    const first = rows[0];
    expect(first.find("summary").text()).toContain("cl/fmt");
    expect(first.text()).toContain("доказательства (1)");
    expect(first.text()).toContain("probe: fmt отстаёт");
    expect(first.text()).toContain("сайт: a.rs:442");
    expect(rows[1].text()).toContain("сайт не указан");
    w.unmount();
  });

  it("hides other saved groups under a filter and restores them on «всё»", async () => {
    const w = mountPanel();
    const select = w.find("select");
    // «всё» + each saved group, counts in the label, no invented groups.
    expect(select.findAll("option").map((o) => o.text())).toEqual([
      "всё (3)",
      "БЬЁТ ПО ВЕРДИКТУ (2)",
      "НЮАНСЫ (1)",
    ]);
    await select.setValue("nuances");
    expect(w.findAll(".qr-group-title").map((h) => h.text())).toEqual(["НЮАНСЫ (1)"]);
    expect(w.text()).toContain("бьёт по вердикту: 2"); // counters untouched
    await select.setValue(ALL_GROUPS);
    expect(w.findAll(".qr-group-title").map((h) => h.text())).toEqual([
      "БЬЁТ ПО ВЕРДИКТУ (2)",
      "НЮАНСЫ (1)",
    ]);
    w.unmount();
  });

  it("folds a stored point into the draft and sends it as the return rationale", async () => {
    const w = mountPanel();
    const actions = w.findComponent(QualityGateActions);
    expect(actions.props("draft")).toHaveLength(0);
    await w.findAll("button.add")[0].trigger("click");
    const draft = w.findComponent(QualityGateActions).props("draft") as { requirement: string; site: string }[];
    expect(draft).toHaveLength(1);
    expect(draft[0].requirement).toBe("cargo fmt дрейф"); // verbatim stored summary
    expect(draft[0].site).toBe("a.rs:442");

    await w.find("button.hold.rework").trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    const emits = w.emitted("apply");
    expect(emits).toHaveLength(1);
    const [kind, text] = emits?.[0] as [string, string];
    expect(kind).toBe("return");
    expect(text).toContain("cargo fmt дрейф");
    expect(text).toContain("сайт: a.rs:442");
    w.unmount();
  });
});
