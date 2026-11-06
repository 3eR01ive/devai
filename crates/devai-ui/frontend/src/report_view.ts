// Presentation decisions for the quality-report panel (REQ-QH-06, rework
// R2/R3). ONE pure module, no Vue and no store: what the panel shows is
// decided here and the template only lays it out, so the branches (stored
// form, damaged form, legacy flat text, absence) are testable without a
// browser — `tests/report_view.test.ts` drives them with `node --test`.
//
// The API already distinguishes the outcomes (`QualityReportView.source`:
// human_form | flat_text | corrupt_form | corrupt_body | absent), and a
// structurally damaged `human_view` keeps its honest marker in the platform's
// `report_text`. This module never trusts `report.report.human_view` just
// because it is present: it re-checks the SHAPE, because a broken form made
// the panel throw (headline.score.toFixed on a number that is not one).
import type {
  HumanForm,
  HumanFormGroup,
  HumanFormItem,
  QualityReportView,
  ReportFreshness,
} from "./types";

/** What the panel renders — exactly one branch, decided from stored facts. */
export type ReportBody =
  | { kind: "form"; form: HumanForm }
  | { kind: "damaged"; marker: string; text: string | null }
  | { kind: "flat"; text: string }
  | { kind: "absent"; text: string };

/** The number format of a point lives ONCE, in Rust (`fmt_num`): the panel
 * prints the stored `headline.numbers` and `item.line` verbatim, so two
 * surfaces can never disagree on a precision (rework R3). Nothing here
 * re-formats a score, a weight or an importance. */

const GROUP_KEYS = ["blocks", "nuances", "anchors", "unchecked"];

function isNum(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

function isNumOrNull(v: unknown): v is number | null {
  return v === null || isNum(v);
}

function isStr(v: unknown): v is string {
  return typeof v === "string";
}

function isItem(v: unknown): v is HumanFormItem {
  if (!v || typeof v !== "object") return false;
  const it = v as Record<string, unknown>;
  if (!isStr(it.id) || !isStr(it.kind) || !isStr(it.summary) || !isStr(it.site)) return false;
  if (!isStr(it.line)) return false;
  if (!isNumOrNull(it.score) || !isNumOrNull(it.weight) || !isNumOrNull(it.importance)) {
    return false;
  }
  if (it.ratified_by !== undefined && !isStr(it.ratified_by)) return false;
  if (it.reason !== undefined && !isStr(it.reason)) return false;
  if (it.evidence !== undefined) {
    if (!Array.isArray(it.evidence) || !it.evidence.every(isStr)) return false;
  }
  return true;
}

/**
 * The shape guard: a stored form is usable only when every field the panel
 * reads is there and of the declared type. Returns the form, or a sentence
 * naming the first thing that is wrong (shown as the honest marker instead of
 * a crash or a silent empty panel).
 */
export function validateHumanForm(v: unknown): { form: HumanForm } | { error: string } {
  if (!v || typeof v !== "object") return { error: "форма отсутствует или не объект" };
  const f = v as Record<string, unknown>;
  if (!isNum(f.version)) return { error: "нет версии формы" };
  if (!isStr(f.lang)) return { error: "нет языка формы" };
  const h = f.headline as Record<string, unknown> | undefined;
  if (!h || typeof h !== "object") return { error: "нет вердикт-строки (headline)" };
  if (!isStr(h.verdict) || !isStr(h.machine_verdict)) return { error: "в headline нет вердикта" };
  if (!isNum(h.score) || !isNum(h.theta)) return { error: "в headline нет чисел S/порога" };
  if (!isStr(h.numbers)) return { error: "в headline нет готовой строки чисел" };
  if (!isStr(h.basis)) return { error: "в headline нет basis" };
  const c = h.counts as Record<string, unknown> | undefined;
  if (!c || typeof c !== "object") return { error: "в headline нет счётчиков групп" };
  for (const k of GROUP_KEYS) {
    if (!isNum(c[k])) return { error: `в счётчиках нет «${k}»` };
  }
  if (!Array.isArray(f.groups)) return { error: "нет групп формы" };
  for (const g of f.groups) {
    if (!g || typeof g !== "object") return { error: "группа формы не объект" };
    const gr = g as Record<string, unknown>;
    if (!isStr(gr.key) || !GROUP_KEYS.includes(gr.key)) return { error: "у группы нет ключа" };
    if (!isStr(gr.title) || !isStr(gr.lead)) return { error: `у группы «${gr.key}» нет заголовка` };
    if (!Array.isArray(gr.items)) return { error: `у группы «${gr.key}» нет списка пунктов` };
    for (const it of gr.items) {
      if (!isItem(it)) return { error: `у группы «${gr.key}» пункт без обязательных полей` };
    }
  }
  if (!isNum(f.blocked_weight) || !isNum(f.max_blocked_weight)) {
    return { error: "нет веса непроверенного" };
  }
  return { form: f as unknown as HumanForm };
}

/**
 * Which branch the panel shows for a stored report — the SAME decision the
 * API made (`source`), re-checked against the body because the UI must not
 * crash on a damaged form. Nothing is regrouped or recomputed: a valid form
 * is passed through verbatim, a damaged one becomes an explicit marker.
 */
export function presentReport(report: QualityReportView): ReportBody {
  const text = report.report_text;
  if (report.source === "human_form") {
    const body = report.report as { human_view?: unknown } | null | undefined;
    const checked = validateHumanForm(body?.human_view);
    if ("form" in checked) return { kind: "form", form: checked.form };
    return {
      kind: "damaged",
      marker: `⚠ сохранённая форма отчёта ${shortArtifact(report.artifact)} повреждена (${checked.error}) — показывать её как форму нельзя`,
      text: text ?? null,
    };
  }
  if (report.source === "flat_text") {
    return { kind: "flat", text: text ?? "" };
  }
  // corrupt_form / corrupt_body / absent: the platform's own marker travels in
  // report_text; show it verbatim, never as «отчёта нет».
  if (text) return { kind: "flat", text };
  return { kind: "absent", text: "(отчёт без рендеринга — пустое тело)" };
}

/** The short artifact label used in the damaged-form marker. */
export function shortArtifact(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

/** The freshness banner of the panel, as one decision: «no banner» or the
 * ready head (the platform's marker, quoted verbatim) plus its fact line.
 * The panel template lays out exactly this — no branch of its own (rework R5:
 * the stale-banner branch is covered by `tests/report_view.test.ts`). */
export type FreshnessBanner =
  | { stale: false }
  | { stale: true; head: string; detail: string };

export function freshnessBanner(
  freshness: ReportFreshness | null | undefined,
  reportStatus: string,
): FreshnessBanner {
  if (!freshness || !freshness.stale) return { stale: false };
  const parts = [`статус отчёта ${reportStatus}`, `фаза ${freshness.task_state}`];
  if (freshness.execution) {
    parts.push(`execution ${shortArtifact(freshness.execution)} (${freshness.execution_status})`);
  }
  return { stale: true, head: freshness.marker, detail: parts.join(" · ") };
}

// ---------------------------------------------------------------------------
// The panel's remaining render decisions (rework R4): which groups print, the
// «пройдено без замечаний» note, the name-only anchor rows and the collapsed
// evidence layer. They live here — not in the template — so `node --test` can
// cover every branch without a browser. No wording and no number is produced
// here: the strings are the stored Rust ones, printed verbatim.
// ---------------------------------------------------------------------------

/** The groups the panel prints: an empty group is omitted, exactly like the
 * CLI renderer's conditional sections. */
export function visibleGroups(form: HumanForm): HumanFormGroup[] {
  return form.groups.filter((g) => g.items.length > 0);
}

/** «пройдено без замечаний: N» prints only when the form counted such points
 * (a v1 form has no `passed` field at all). */
export function showsPassedNote(form: HumanForm): boolean {
  return (form.headline.counts.passed ?? 0) > 0;
}

/** Anchors are name-only rows: the stored line and nothing else; every other
 * group also prints summary, reason and the evidence layer. */
export function isNameOnlyGroup(group: HumanFormGroup): boolean {
  return group.key === "anchors";
}

/** The collapsed proof layer exists exactly when evidence was stored. */
export function hasEvidence(it: HumanFormItem): boolean {
  return (it.evidence?.length ?? 0) > 0;
}
