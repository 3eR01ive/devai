// Branch tests for the report panel (rework R3). These drive the SAME pure
// module the panel template uses (`src/report_view.ts`), so the render
// decisions — stored form, damaged form, legacy flat text, absence, the
// anchor row, the freshness banner input — are covered without a browser.
//
// Run: `npm test` (node --test on the TS sources; Node strips the types).
import assert from "node:assert/strict";
import test from "node:test";
import { freshnessBanner, hasEvidence, presentReport, showsPassedNote, validateHumanForm, visibleGroups } from "../src/report_view.ts";
import type { QualityReportView, ReportFreshness } from "../src/types.ts";

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
      numbers: "S=0.820/1.00, порог 0.900",
      basis: "красные линии: cl/fmt=0.0<floor 0.9",
      counts: { blocks: 1, nuances: 2, anchors: 1, unchecked: 1 },
    },
    groups: [
      {
        key: "blocks",
        title: "БЬЁТ ПО ВЕРДИКТУ",
        lead: "решается тобой",
        items: [
          { id: "cl/fmt", kind: "mechanical", summary: "cargo fmt дрейф", line: "cl/fmt · балл 0.000 · вес 0.050 · важность 0.050 · a.rs:442", site: "a.rs:442", score: 0, weight: 0.05, importance: 0.05 },
        ],
      },
      {
        key: "nuances",
        title: "НЮАНСЫ",
        lead: "",
        items: [
          { id: "n/one", kind: "scored", summary: "мелочь", line: "n/one · балл 0.900 · вес 0.100 · важность 0.010 · b.rs:1", site: "b.rs:1", score: 0.9, weight: 0.1, importance: 0.01, evidence: ["x"] },
        ],
      },
      {
        key: "anchors",
        title: "ЯКОРЯ",
        lead: "",
        items: [
          { id: "anch/a", kind: "anchor", summary: "закреплено", line: "  · anch/a — закреплено: решение 01a1b1b8-dec (guerrero)", site: "", score: 1, weight: 0, importance: 0, ratified_by: "решение 01a1b1b8-dec (guerrero)" },
        ],
      },
      {
        key: "unchecked",
        title: "НЕ ПРОВЕРЕНО",
        lead: "",
        items: [{ id: "brk/x", kind: "blocked", summary: "проба не прошла", line: "brk/x · балл — · вес 0.100", site: "", score: null, weight: 0.1, importance: null, reason: "нет пробы" }],
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

function freshness(over: Partial<ReportFreshness>): ReportFreshness {
  return {
    stale: true,
    marker: "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: возврат без ре-аудита; последний прогон 01a1b29e COMPLETED 2026-11-06 20:48:16Z",
    report_seq: 1,
    report_status: "STALE",
    task_state: "REWORK",
    execution: "01a1b29e-8adf-7113-9243-3d217f1a74a4",
    execution_status: "RUNNING",
    ...over,
  };
}

test("a current report shows no stale banner", () => {
  assert.deepEqual(freshnessBanner(null, "CURRENT"), { stale: false });
  assert.deepEqual(
    freshnessBanner(freshness({ stale: false }), "CURRENT"),
    { stale: false },
    "a non-stale fact never draws a banner, whatever its marker says",
  );
});

// R5/R6: the panel's banner text is pinned byte-for-byte for the three
// freshness formulations — a wording change is visible in the diff instead of
// silently shipping.
test("the stale banner pins the running re-audit wording", () => {
  const b = freshnessBanner(freshness({}), "STALE");
  assert.equal(b.stale, true);
  if (!b.stale) return;
  assert.equal(
    b.head,
    "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: возврат без ре-аудита; последний прогон 01a1b29e COMPLETED 2026-11-06 20:48:16Z",
  );
  assert.equal(
    b.detail,
    "статус отчёта STALE · фаза REWORK · execution 01a1b29e (RUNNING)",
  );
  // The FIRST thing a human reads is the plain sentence, not the marker.
  assert.match(b.plain, /^Этот отчёт устарел — решать по нему уже нельзя/);
  assert.match(b.plain, /Новый аудит уже идёт — дождись его отчёта/);
  assert.doesNotMatch(b.plain, /STALE|REWORK|RUNNING/);
});

test("the stale banner pins the «no execution» wording without an id", () => {
  const b = freshnessBanner(
    freshness({
      marker: "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: ре-аудит ещё не запущен",
      execution: null,
      execution_status: null,
    }),
    "STALE",
  );
  assert.equal(b.stale, true);
  if (!b.stale) return;
  assert.equal(b.head, "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: ре-аудит ещё не запущен");
  assert.equal(b.detail, "статус отчёта STALE · фаза REWORK");
  assert.match(b.plain, /Дождись нового отчёта аудита/);
  assert.doesNotMatch(b.plain, /аудит уже идёт/, "nothing is claimed about a run the store did not record");
});

// R6: the visible rows the panel prints ARE the stored Rust `line` strings —
// the panel adds no wording of its own (no second anchor sentence, no second
// number precision).
test("the panel prints the stored item rows verbatim", () => {
  const body = presentReport(view({ report: { human_view: form() } }));
  assert.equal(body.kind, "form");
  if (body.kind !== "form") return;
  const lines = body.form.groups.flatMap((g) => g.items.map((it) => it.line));
  assert.deepEqual(lines, [
    "cl/fmt · балл 0.000 · вес 0.050 · важность 0.050 · a.rs:442",
    "n/one · балл 0.900 · вес 0.100 · важность 0.010 · b.rs:1",
    "  · anch/a — закреплено: решение 01a1b1b8-dec (guerrero)",
    "brk/x · балл — · вес 0.100",
  ]);
  assert.equal(body.form.headline.numbers, "S=0.820/1.00, порог 0.900");
});

// R4 (rework): the panel's own render branches — which groups print, the
// «пройдено» note, name-only anchor rows and the collapsed evidence layer —
// are decisions of this module, so `node --test` covers them.
test("empty groups are not printed while their stored form stays intact", () => {
  const f = form() as any;
  const full = presentReport(view({ report: { human_view: f } }));
  assert.equal(full.kind, "form");
  if (full.kind !== "form") return;
  assert.equal(visibleGroups(full.form).length, 4);

  const stripped = {
    ...full.form,
    groups: full.form.groups.map((g, i) => (i === 3 ? { ...g, items: [] } : g)),
  };
  assert.deepEqual(
    visibleGroups(stripped).map((g) => g.key),
    ["blocks", "nuances", "anchors"],
    "a group with no items prints no heading — but the stored form is unchanged",
  );
  assert.equal(full.form.groups[3].items.length, 1, "no read-time re-derivation happened");
});

test("the «пройдено без замечаний» note appears only when points were counted", () => {
  const f = form() as any;
  assert.equal(showsPassedNote(f), false, "no passed field: nothing to print");
  f.headline.counts.passed = 0;
  assert.equal(showsPassedNote(f), false);
  f.headline.counts.passed = 3;
  assert.equal(showsPassedNote(f), true);
});

test("the collapsed evidence layer exists exactly when evidence was stored", () => {
  assert.equal(hasEvidence({ id: "a", kind: "scored", summary: "s", line: "l", site: "", score: 1, weight: 0, importance: 0 }), false);
  assert.equal(hasEvidence({ id: "a", kind: "scored", summary: "s", line: "l", site: "", score: 1, weight: 0, importance: 0, evidence: [] }), false);
  assert.equal(hasEvidence({ id: "a", kind: "scored", summary: "s", line: "l", site: "", score: 1, weight: 0, importance: 0, evidence: ["probe output"] }), true);
});
