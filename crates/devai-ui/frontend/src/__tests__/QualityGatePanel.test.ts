// The QA report panel at the component seam (REQ-SAY-01..06): the first layer
// a human reads — the appeal, the plain-words findings, «не проверено» apart
// and the filled return draft — and the details layer where the machine ids,
// the verdict token and every figure live. IPC is not involved: the panel
// renders the props it is given.
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import QualityGateActions from "../components/quality/QualityGateActions.vue";
import QualityGatePanel from "../components/quality/QualityGatePanel.vue";
import { ACCEPT_LABEL, RETURN_LABEL } from "../quality_gate_view";
import { HOLD_MS } from "../signature_view";
import type { FirstLayer, QualityGateView, QualityReportView } from "../types";

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

/** The platform's first layer for the fixture: two blocking points, one
 *  nuance, one unchecked check. The wording is the platform's, printed as is. */
function firstLayer(): FirstLayer {
  const it = (id: string, what: string, site: string, note: string, evidence?: string[]) => ({
    id, what, site, note, line: `${id} · балл 0.000 · вес 0.100`, evidence,
  });
  return {
    found: "Принять как есть нельзя: есть замечания, которые мешают.",
    score_line: "сохранённый балл работы: 0.820",
    findings: [
      it("cl/fmt", "cargo fmt дрейф", "a.rs:442", "без этого работу принять нельзя", ["probe: fmt отстаёт"]),
      it("n/one", "мелкая придирка", "", "на будущее — принять не мешает"),
    ],
    unchecked: [
      {
        id: "brk/x",
        what: "проверка не прошла",
        site: "",
        note: "нужен повторный прогон: проверке не хватило условий",
        missing: "probe could not run",
        line: "brk/x · балл — · вес 0.050",
      },
    ],
    unchecked_lead: "это не недостатки работы, а проверки, которых не хватило",
  };
}

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
    first_layer: firstLayer(),
    report: {
      human_view: {
        version: 2,
        lang: "ru",
        headline: {
          verdict: "НЕ ПРОШЁЛ", machine_verdict: "FAIL", score: 0.82, theta: 0.9,
          numbers: "S=0.820/1.00, порог 0.900", basis: "красные линии: cl/fmt",
          counts: { blocks: 1, nuances: 1, anchors: 0, unchecked: 1 },
        },
        groups: [
          {
            key: "blocks", title: "БЬЁТ ПО ВЕРДИКТУ", lead: "решается тобой",
            items: [one("cl/fmt", "cargo fmt дрейф", "a.rs:442", ["probe: fmt отстаёт"])],
          },
          {
            key: "nuances", title: "НЮАНСЫ", lead: "",
            items: [one("n/one", "мелкая придирка", "")],
          },
          {
            key: "unchecked", title: "НЕ ПРОВЕРЕНО", lead: "",
            items: [one("brk/x", "проверка не прошла", "")],
          },
          {
            key: "anchors", title: "ЯКОРЯ", lead: "",
            items: [{
              id: "anch/a", kind: "anchor", summary: "ратифицированный инвариант",
              site: "", line: "  · anch/a — закреплено: решение 01a1b1b8-dec (guerrero)",
              score: 1, weight: 0, importance: 0, ratified_by: "решение 01a1b1b8-dec (guerrero)",
            }],
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

function mountPanel(g: QualityGateView | null = gate()) {
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
  it("opens with what the human is asked, what the audit found, and both consequences", () => {
    const w = mountPanel();
    const head = w.find(".qr-head").text();
    expect(head).toContain("принять эту работу или вернуть её на переделку");
    expect(head).toContain("Принять как есть нельзя");
    // The appeal and the assessment know no machine token and no figure; the
    // saved score stands beside them as its own element (REQ-SAY-02 v4).
    expect(head).not.toMatch(/FAIL|INCONCLUSIVE|PASS/);
    expect(w.find(".qr-appeal").text()).not.toMatch(/\d/);
    expect(w.find(".qr-found").text()).not.toMatch(/\d/);
    expect(w.find(".qr-score").text()).toBe("сохранённый балл работы: 0.820");
    // Both buttons are signed by their consequence.
    expect(w.text()).toContain(ACCEPT_LABEL);
    expect(w.text()).toContain(RETURN_LABEL);
    w.unmount();
  });

  it("puts the machine verdict, the numbers and the counters behind «подробности»", () => {
    const w = mountPanel();
    const details = w.find("details.qr-details");
    expect(details.exists()).toBe(true);
    expect((details.element as HTMLDetailsElement).open).toBe(false);
    expect(details.text()).toContain("ВЕРДИКТ: НЕ ПРОШЁЛ (FAIL)");
    expect(details.text()).toContain("S=0.820/1.00, порог 0.900");
    expect(details.text()).toContain("бьёт по вердикту: 1");
    expect(details.text()).toContain("БЬЁТ ПО ВЕРДИКТУ (1)");
    w.unmount();
  });

  it("reads every finding as a human phrase with its place and its weight on the verdict", () => {
    const w = mountPanel();
    const rows = w.findAll(".qr-findings details.qr-item");
    expect(rows).toHaveLength(2);
    const first = rows[0];
    const summary = first.find("summary").text();
    expect(summary).toContain("cargo fmt дрейф");
    expect(summary).toContain("где: a.rs:442");
    expect(summary).toContain("без этого работу принять нельзя");
    // The machine row is in the expansion, not in the headline.
    expect(summary).not.toContain("cl/fmt");
    expect(first.text()).toContain("cl/fmt · балл 0.000");
    // A point with no site says so in words.
    expect(rows[1].find("summary").text()).toContain("место не указано");
    expect(rows[1].find("summary").text()).toContain("принять не мешает");
    // Evidence stays available in its own expansion.
    expect(first.text()).toContain("probe: fmt отстаёт");
    w.unmount();
  });

  it("keeps «не проверено» in its own section: what was missing and what to do", () => {
    const w = mountPanel();
    const section = w.find(".qr-unchecked");
    expect(section.exists()).toBe(true);
    // The heading names the section, not a count: figures live in «подробности».
    expect(section.find("h4").text()).toBe("Не проверено — чего не хватило");
    expect(section.find("h4").text()).not.toMatch(/\d/);
    expect(section.text()).toContain("не недостатки работы");
    expect(section.text()).toContain("чего не хватило: probe could not run");
    expect(section.text()).toContain("что делать: нужен повторный прогон");
    // Not mixed into the findings list.
    expect(w.find(".qr-findings").text()).not.toContain("проверка не прошла");
    w.unmount();
  });

  it("hands the actions a draft already filled with every visible finding", () => {
    const w = mountPanel();
    const actions = w.findComponent(QualityGateActions);
    const draft = actions.props("draft") as { requirement: string; site: string }[];
    expect(draft.map((d) => d.requirement)).toEqual(["cargo fmt дрейф", "мелкая придирка"]);
    expect(draft.map((d) => d.site)).toEqual(["a.rs:442", ""]);
    expect(actions.props("draftNote")).toContain("сними те, что не надо возвращать");
    // «не проверено» is offered as one click, not prefilled.
    expect(actions.props("unchecked")).toHaveLength(1);
    w.unmount();
  });

  it("takes a point out of the filled draft and sends the rest", async () => {
    const w = mountPanel();
    // The finding row's toggle removes it: the operator trims, not assembles.
    await w.findAll(".qr-findings button.add")[1].trigger("click");
    let draft = w.findComponent(QualityGateActions).props("draft") as { item_id: string }[];
    expect(draft.map((d) => d.item_id)).toEqual(["cl/fmt"]);

    await w.find("button.hold.rework").trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    const emits = w.emitted("apply");
    expect(emits).toHaveLength(1);
    const [kind, text] = emits?.[0] as [string, string];
    expect(kind).toBe("return");
    expect(text).toContain("cargo fmt дрейф");
    expect(text).toContain("где: a.rs:442");
    expect(text).not.toContain("мелкая придирка");
    w.unmount();
  });

  it("says in words why there is no decision instead of offering a dead button", () => {
    const w = mountPanel(
      gate({
        may_accept: false,
        may_return: false,
        accept_denied: { code: "no_current_report", text: "нет текущего отчёта" },
        return_denied: { code: "not_qa", text: "задача не на шаге решения" },
      }),
    );
    expect(w.find(".qr-appeal").text()).toContain("ни принять, ни вернуть нельзя");
    expect(w.find(".qr-why").text()).toContain("нет текущего отчёта");
    w.unmount();
  });

  it("reads the anchors like every finding: human phrase first, service row in the expansion", () => {
    const w = mountPanel();
    const anchors = w.find("details.qr-details");
    // The machine id is present — in the details layer, under the row.
    const rows = anchors.findAll(".qr-group").filter((g) => g.text().includes("anch/a"));
    expect(rows).toHaveLength(1);
    const row = rows[0].find("details.qr-item");
    expect(row.find("summary").text()).toContain("ратифицированный инвариант");
    expect(row.find("summary").text()).not.toContain("anch/a");
    expect(row.text()).toContain("закреплено: решение 01a1b1b8-dec (guerrero)");
    w.unmount();
  });

  it("explains an outdated report in plain words and offers no accept/return", () => {
    const r = report();
    r.freshness = {
      stale: true,
      marker: "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: ре-аудит ещё не запущен",
      report_seq: 1,
      report_status: "STALE",
      task_state: "REWORK",
      execution: null,
      execution_status: null,
    };
    const w = mount(QualityGatePanel, {
      props: { report: r, reportError: null, gate: gate(), busy: false, error: null },
    });
    // What happened and what to do comes first, in human words.
    expect(w.find(".qr-stale").text()).toContain("решать по нему уже нельзя");
    expect(w.find(".qr-stale").text()).toContain("Дождись нового отчёта аудита");
    // No appeal to accept or return, and no buttons at all.
    expect(w.find(".qr-appeal").exists()).toBe(false);
    expect(w.findComponent(QualityGateActions).exists()).toBe(false);
    // The machine marker and the store facts stay reachable in the details.
    const details = w.find("details.qr-details").text();
    expect(details).toContain("УСТАРЕЛ");
    expect(details).toContain("фаза REWORK");
    // The report itself is still readable.
    expect(w.find(".qr-findings").text()).toContain("cargo fmt дрейф");
    w.unmount();
  });

  it("shows a legacy report honestly, without dressing it up as the new layer", () => {
    const r = report();
    r.first_layer = null;
    r.source = "flat_text";
    r.report = {};
    r.report_text = "ВЕРДИКТ: ПРОШЁЛ (PASS) — S=0.950/1.00";
    const w = mount(QualityGatePanel, {
      props: { report: r, reportError: null, gate: gate(), busy: false, error: null },
    });
    expect(w.text()).toContain("человеческой подачи в этом отчёте нет");
    expect(w.text()).toContain("ВЕРДИКТ: ПРОШЁЛ (PASS)");
    expect(w.find(".qr-findings").exists()).toBe(false);
    // The draft is empty and the screen says there is nothing to return.
    expect(w.findComponent(QualityGateActions).props("draftNote")).toContain("возвращать нечего");
    w.unmount();
  });
});
