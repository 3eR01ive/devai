// Pure rules of the QA gate's return draft and the report's importance filter
// (REQ-QG-02/03/04/05). Run: `npm test` (node --test on the TS sources).
import assert from "node:assert/strict";
import test from "node:test";
import {
  ACCEPT_LABEL,
  RETURN_LABEL,
  addDraftItem,
  canAccept,
  canReturn,
  composeReturnRationale,
  decisionPrompt,
  draftItemOf,
  draftText,
  editDraftRequirement,
  inDraft,
  prefillDraft,
  removeDraftItem,
  sentPoints,
  siteText,
  toggleDraftItem,
} from "../src/quality_gate_view.ts";
import {
  ALL_GROUPS,
  filterOptions,
  groupsForFilter,
  presentReport,
} from "../src/report_view.ts";
import type { HumanForm, QualityGateView, QualityReportView } from "../src/types.ts";

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

function reportView(form: unknown): QualityReportView {
  return {
    task: "t1",
    artifact: "01a1b2aa-1111-700a-aaaa-111111111111",
    seq: 1,
    contract: "quality-report@5",
    created_at: "2026-11-09T10:00:00Z",
    execution: null,
    status: "CURRENT",
    report: { human_view: form },
    report_text: null,
    source: "human_form",
    freshness: null,
    first_layer: null,
  };
}

function form(): HumanForm {
  return {
    version: 1,
    lang: "ru",
    headline: {
      verdict: "НЕ ПРОШЁЛ",
      machine_verdict: "FAIL",
      score: 0.82,
      theta: 0.9,
      numbers: "S=0.820/1.00, порог 0.900",
      basis: "бьёт по вердикту",
      counts: { blocks: 1, nuances: 1, anchors: 1, unchecked: 1 },
    },
    groups: [
      {
        key: "blocks",
        title: "БЬЁТ ПО ВЕРДИКТУ",
        lead: "",
        items: [
          { id: "cl/fmt", kind: "mechanical", summary: "cargo fmt дрейф", line: "cl/fmt · a.rs:1", site: "a.rs:1", score: 0, weight: 0.05, importance: 0.05 },
          { id: "cl/test", kind: "mechanical", summary: "тест падает", line: "cl/test · b.rs:2", site: "b.rs:2", score: 0, weight: 0.05, importance: 0.05 },
        ],
      },
      {
        key: "nuances",
        title: "НЮАНСЫ",
        lead: "",
        items: [
          { id: "n/one", kind: "scored", summary: "мелочь", line: "n/one · c.rs:3", site: "c.rs:3", score: 0.9, weight: 0.1, importance: 0.01 },
        ],
      },
      {
        key: "anchors",
        title: "ЯКОРЯ",
        lead: "",
        items: [
          { id: "anch/a", kind: "anchor", summary: "закреплено", line: "anch/a", site: "", score: 1, weight: 0, importance: 0 },
        ],
      },
      {
        key: "unchecked",
        title: "НЕ ПРОВЕРЕНО",
        lead: "",
        items: [],
      },
    ],
    blocked_weight: 0.1,
    max_blocked_weight: 0.2,
  };
}

test("a drafted point carries the stored summary verbatim and its site", () => {
  const d = draftItemOf({ id: "cl/fmt", summary: "cargo fmt дрейф", site: "a.rs:1" });
  assert.equal(d.requirement, "cargo fmt дрейф", "prefilled with the stored summary, not a retelling");
  assert.equal(d.site, "a.rs:1");
  assert.equal(siteText(""), "место не указано", "a missing site is named in words");
  assert.equal(siteText("a.rs:1"), "a.rs:1");
});

test("a point is added once; the requirement stays editable and removable", () => {
  const item = { id: "cl/fmt", summary: "cargo fmt дрейф", site: "" };
  let draft = addDraftItem([], item);
  draft = addDraftItem(draft, item);
  assert.equal(draft.length, 1, "a second add of the same point is a no-op");

  draft = editDraftRequirement(draft, "cl/fmt", "выровнять форматирование");
  assert.equal(draft[0].requirement, "выровнять форматирование");
  assert.equal(draft[0].summary, "cargo fmt дрейф", "the stored summary is kept as the record");

  draft = removeDraftItem(draft, "cl/fmt");
  assert.deepEqual(draft, []);
});

test("a point with an empty requirement never reaches the sent text", () => {
  const draft = [
    { key: "a", item_id: "a", summary: "one", site: "a.rs:1", requirement: "  " },
    { key: "b", item_id: "b", summary: "two", site: "", requirement: "исправить два" },
  ];
  assert.deepEqual(sentPoints(draft), [{ requirement: "исправить два", site: "" }]);
  const text = draftText(draft);
  assert.match(text, /1\. исправить два/);
  assert.match(text, /место не указано/);
  assert.doesNotMatch(text, /one/);
});

test("the return text is the manual rationale plus the drafted points", () => {
  const draft = [
    { key: "a", item_id: "a", summary: "one", site: "a.rs:1", requirement: "исправить a" },
  ];
  const text = composeReturnRationale("общий комментарий", draft);
  assert.match(text, /^общий комментарий/);
  assert.match(text, /Пункты возврата:/);
  assert.match(text, /1\. исправить a/);
  assert.match(text, /где: a\.rs:1/);

  // Either half alone is enough; both empty is the only blocked case.
  assert.equal(composeReturnRationale("", draft).trim().length > 0, true);
  assert.equal(composeReturnRationale("только слова", []).trim(), "только слова");
  assert.equal(composeReturnRationale("   ", []).trim(), "");
});

test("acceptance needs a non-empty rationale and the platform's may_accept", () => {
  assert.equal(canAccept(gate(), ""), false);
  assert.equal(canAccept(gate(), "  "), false);
  assert.equal(canAccept(gate(), "принято"), true);
  assert.equal(canAccept(gate({ may_accept: false }), "принято"), false);
  assert.equal(canAccept(null, "принято"), false);
});

test("return can be sent with drafted points alone; an empty text blocks it", () => {
  const draft = [
    { key: "a", item_id: "a", summary: "one", site: "", requirement: "исправить a" },
  ];
  assert.equal(canReturn(gate(), "", []), false);
  assert.equal(canReturn(gate(), "вернуть", []), true);
  assert.equal(canReturn(gate(), "", draft), true, "the draft itself is a non-empty rationale");
  assert.equal(canReturn(gate({ may_return: false }), "", draft), false);
});

test("both buttons are signed by their consequence, in words — no internal state", () => {
  // REQ-SAY-01 AC3/AC4: what becomes of the task, said plainly; a machine
  // token (ACCEPTED / REWORK / PASS...) never appears in the first layer.
  assert.match(ACCEPT_LABEL, /^Принять — работа считается сделанной$/);
  assert.match(RETURN_LABEL, /^Вернуть в доработку — задача пойдёт на переделку/);
  for (const label of [ACCEPT_LABEL, RETURN_LABEL]) {
    assert.doesNotMatch(label, /ACCEPTED|REWORK|INCONCLUSIVE|PASS|FAIL/);
  }
});

test("the appeal line says what the human is asked, and why when nothing is open", () => {
  // Both acts open: the appeal names them, no reason line.
  const both = decisionPrompt(gate());
  assert.match(both.head, /принять эту работу или вернуть её на переделку/);
  assert.equal(both.why, "");

  // Only one act: the other one's refusal is quoted from the platform.
  const onlyAccept = decisionPrompt(
    gate({ may_return: false, return_denied: { code: "audit_running", text: "идёт аудит" } }),
  );
  assert.match(onlyAccept.head, /принять эту работу/);
  assert.doesNotMatch(onlyAccept.head, /вернуть/);
  assert.match(onlyAccept.why, /вернуть нельзя: идёт аудит/);

  // Neither act while an audit runs: words, not a dead button.
  const running = decisionPrompt(
    gate({ may_accept: false, may_return: false, audit_running: true }),
  );
  assert.match(running.head, /аудит ещё идёт/);
  assert.match(running.why, /принять или вернуть нельзя/);

  // Neither act for a named refusal: the platform's own reason is quoted.
  const refused = decisionPrompt(
    gate({
      may_accept: false,
      may_return: false,
      accept_denied: { code: "no_current_report", text: "нет текущего отчёта" },
    }),
  );
  assert.match(refused.why, /принять нельзя: нет текущего отчёта/);

  // No gate at all: nothing is invented about the future.
  const absent = decisionPrompt(null);
  assert.match(absent.head, /не предлагает такого выбора/);
});

// The findings the platform hands the panel: two decision items and one that
// does not block; «не проверено» is a separate list.
const findings = [
  { id: "cl/fmt", what: "cargo fmt дрейф", site: "a.rs:1", stake: "без этого принять нельзя", line: "cl/fmt · балл 0.000" },
  { id: "cl/test", what: "тест падает", site: "", stake: "без этого принять нельзя", line: "cl/test · балл 0.000" },
  { id: "n/one", what: "мелкая придирка", site: "c.rs:3", stake: "на будущее — принять не мешает", line: "n/one · балл 0.900" },
];

test("the return draft comes filled with every visible finding, in its stored words", () => {
  // REQ-SAY-05: nothing to assemble by hand — all findings, phrase and place.
  const draft = prefillDraft(findings);
  assert.equal(draft.length, 3, "every finding the human sees is in the draft");
  assert.deepEqual(
    draft.map((d) => d.requirement),
    ["cargo fmt дрейф", "тест падает", "мелкая придирка"],
    "the requirement IS the stored human phrase",
  );
  assert.deepEqual(
    draft.map((d) => d.site),
    ["a.rs:1", "", "c.rs:3"],
    "the place travels with the point",
  );
  // «не проверено» is not a defect: it is added by one click, not prefilled.
  assert.equal(inDraft(draft, "brk/x"), false);
  const withHole = toggleDraftItem(draft, { id: "brk/x", summary: "проверка не прошла", site: "" });
  assert.equal(inDraft(withHole, "brk/x"), true);
  // The same click takes a prefilled point out — the operator removes extra.
  assert.equal(inDraft(toggleDraftItem(withHole, { id: "cl/fmt", summary: "cargo fmt дрейф", site: "a.rs:1" }), "cl/fmt"), false);
  // The sent text carries the points, each with its place.
  const sent = composeReturnRationale("", prefillDraft(findings));
  assert.match(sent, /cl\/fmt|Пункты возврата:/);
  assert.match(sent, /1\. cargo fmt дрейф/);
  assert.match(sent, /где: a\.rs:1/);
  assert.match(sent, /где: место не указано/);
  assert.equal(canReturn(gate(), "", prefillDraft(findings)), true, "return is available out of the box");
});

test("the importance filter offers all + every stored group and hides without rewriting", () => {
  const body = presentReport(reportView(form()));
  assert.equal(body.kind, "form");
  if (body.kind !== "form") return;

  const options = filterOptions(body.form);
  assert.deepEqual(
    options.map((o) => o.key),
    [ALL_GROUPS, "blocks", "nuances", "anchors"],
    "an empty stored group gets no filter row",
  );
  assert.equal(options[0].title, "всё");
  assert.equal(options[0].count, 4);

  const snapshot = JSON.stringify(body.form);
  assert.deepEqual(
    groupsForFilter(body.form, "blocks").map((g) => g.key),
    ["blocks"],
  );
  assert.deepEqual(
    groupsForFilter(body.form, ALL_GROUPS).map((g) => g.key),
    ["blocks", "nuances", "anchors"],
    "«всё» restores every visible group in the stored order",
  );
  assert.deepEqual(
    groupsForFilter(body.form, "nope").map((g) => g.key),
    ["blocks", "nuances", "anchors"],
    "an unknown filter falls back to all, never to an empty panel",
  );
  assert.equal(JSON.stringify(body.form), snapshot, "filtering changed no stored data");
});
