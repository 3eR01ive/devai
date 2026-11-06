// Branch tests for the report panel (rework R3). These drive the SAME pure
// module the panel template uses (`src/report_view.ts`), so the render
// decisions — stored form, damaged form, legacy flat text, absence, the
// anchor row, the freshness banner input — are covered without a browser.
//
// Run: `npm test` (node --test on the TS sources; Node strips the types).
import assert from "node:assert/strict";
import test from "node:test";
import { anchorLine, fmtNum, presentReport, validateHumanForm } from "../src/report_view.ts";
import type { QualityReportView } from "../src/types.ts";

function view(over: Partial<QualityReportView>): QualityReportView {
  return {
    task: "01a1b1d6-e364-7696-a82f-a72b29df68a2",
    artifact: "01a1b2aa-1111-700a-aaaa-111111111111",
    seq: 1,
    contract: "quality-report@5",
    created_at: "2026-11-06T20:00:00Z",
    execution: null,
    status: "CURRENT",
    report: {},
    report_text: null,
    source: "human_form",
    freshness: null,
    ...over,
  };
}

function form(): unknown {
  return {
    version: 1,
    lang: "ru",
    headline: {
      verdict: "НЕ ПРОШЁЛ",
      machine_verdict: "FAIL",
      score: 0.82,
      theta: 0.9,
      basis: "красные линии: cl/fmt=0.0<floor 0.9",
      counts: { blocks: 1, nuances: 2, anchors: 1, unchecked: 1 },
    },
    groups: [
      {
        key: "blocks",
        title: "БЬЁТ ПО ВЕРДИКТУ",
        lead: "решается тобой",
        items: [
          { id: "cl/fmt", kind: "mechanical", summary: "cargo fmt дрейф", site: "a.rs:442", score: 0, weight: 0.05, importance: 0.05 },
        ],
      },
      {
        key: "nuances",
        title: "НЮАНСЫ",
        lead: "",
        items: [
          { id: "n/one", kind: "scored", summary: "мелочь", site: "b.rs:1", score: 0.9, weight: 0.1, importance: 0.01, evidence: ["x"] },
        ],
      },
      {
        key: "anchors",
        title: "ЯКОРЯ",
        lead: "",
        items: [
          { id: "anch/a", kind: "anchor", summary: "закреплено", site: "", score: 1, weight: 0, importance: 0, ratified_by: "guerrero" },
        ],
      },
      {
        key: "unchecked",
        title: "НЕ ПРОВЕРЕНО",
        lead: "",
        items: [{ id: "brk/x", kind: "blocked", summary: "проба не прошла", site: "", score: null, weight: 0.1, importance: null, reason: "нет пробы" }],
      },
    ],
    blocked_weight: 0.1,
    max_blocked_weight: 0.2,
  };
}

test("a valid stored form is passed through verbatim (no regrouping)", () => {
  const body = presentReport(view({ report: { human_view: form() } }));
  assert.equal(body.kind, "form");
  if (body.kind !== "form") return;
  assert.deepEqual(
    body.form.groups.map((g) => g.key),
    ["blocks", "nuances", "anchors", "unchecked"],
    "the stored group order is preserved",
  );
  assert.equal(body.form.groups[0].items[0].importance, 0.05);
});

test("a structurally damaged form yields an honest marker, not a crash", () => {
  // headline.score is a string and groups is an object: the shape the panel
  // used to call .toFixed(3) / .filter on.
  const damaged = { version: "not-a-number", headline: 42, groups: {} };
  const body = presentReport(
    view({ report: { human_view: damaged }, report_text: "⚠ форма отчёта повреждена, показан плоский текст" }),
  );
  assert.equal(body.kind, "damaged");
  if (body.kind !== "damaged") return;
  assert.match(body.marker, /повреждена/);
  assert.match(body.marker, /011\.?|01a1b2aa/, "the marker names the artifact");
  assert.match(body.text ?? "", /повреждена/, "the platform's flat text stays visible");
});

test("a form missing a required field fails the guard with the field named", () => {
  const broken = form() as Record<string, any>;
  delete broken.headline.basis;
  const checked = validateHumanForm(broken);
  assert.ok("error" in checked);
  if (!("error" in checked)) return;
  assert.match(checked.error, /basis/);
});

test("a legacy report renders its stored flat text byte for byte", () => {
  const text = "## §2.6 wall of text\n[check/x] evidence";
  const body = presentReport(view({ source: "flat_text", report_text: text }));
  assert.deepEqual(body, { kind: "flat", text });
});

test("a corrupt body surfaces the platform marker, never 'no report'", () => {
  const marker = "⚠ отчёт качества 01a1b2aa повреждён: тело блоба не разбирается как JSON";
  const body = presentReport(view({ source: "corrupt_body", report_text: marker }));
  assert.deepEqual(body, { kind: "flat", text: marker });
});

test("an absent rendering keeps an explicit fact instead of an empty panel", () => {
  const body = presentReport(view({ source: "absent", report_text: null }));
  assert.equal(body.kind, "absent");
});

test("the anchor row names the check and who ratified it", () => {
  const anchor = form() as any;
  const item = anchor.groups[2].items[0];
  assert.equal(anchorLine(item), "anch/a — закреплено: guerrero");
  assert.match(anchorLine({ ...item, ratified_by: undefined }), /отсылка на решение не указана/);
});

test("one number precision for score, weight and importance", () => {
  assert.equal(fmtNum(0.05), "0.050");
  assert.equal(fmtNum(0.9), "0.900");
});
