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

// ---------------------------------------------------------------------------
// The report WITHOUT a human first layer (rework R1). Such a body is not
// evidence that the audit found nothing: the composition of its findings is
// UNKNOWN (an old flat text, a damaged form, a body that does not parse), and
// the panel must say so instead of printing «недостатков нет». Every branch
// below is decided from the stored `source` token the API publishes — the
// screen names what is missing and what that means for the decision, and hands
// the return draft ONE honest general reason («состав замечаний не прочитан»)
// so a return does not arrive empty.
// ---------------------------------------------------------------------------

/** Which way the human layer is unavailable — one branch per stored fact. */
export type FirstLayerGapKind =
  | "legacy"
  | "damaged_form"
  | "broken_body"
  | "absent"
  | "not_published"
  | "unknown";

/** Why there is no human layer, and what the human can do about it. */
export interface FirstLayerGap {
  kind: FirstLayerGapKind;
  /** What exactly is unavailable — the first thing the panel says. */
  what: string;
  /** What that means for the decision; never «findings absent». */
  means: string;
  /** One line about the draft that comes WITH this report. */
  draft_note: string;
  /** The general reason the (non-empty) draft carries by default. */
  draft: string;
}

/** The one wording of an unread composition — the draft's common reason. */
export const UNREAD_COMPOSITION = "состав замечаний не прочитан";

/** The stable draft key of that general reason (never a stored item id). */
export const UNREAD_DRAFT_ID = "unread/composition";

const GAP_LEGACY: FirstLayerGap = {
  kind: "legacy",
  what: "В этом отчёте нет человеческого разбора: он сохранён в старом формате, одной строкой текста.",
  means:
    "Что именно нашёл аудит и какие замечания он выставил, по такому отчёту по пунктам не прочитать — " +
    "состав замечаний неизвестен, так что судить, всё ли чисто, по нему нельзя.",
  draft_note:
    "черновик не пустой: в нём одно общее основание — отчёт старого формата, пункты замечаний не прочитаны.",
  draft: `${UNREAD_COMPOSITION}: отчёт старого формата, человеческого разбора в нём нет`,
};

const GAP_DAMAGED_FORM: FirstLayerGap = {
  kind: "damaged_form",
  what: "Человеческий разбор этого отчёта повреждён: сохранённая форма не читается.",
  means:
    "Ни вердикта по пунктам, ни замечаний из неё не достать — состав замечаний неизвестен, " +
    "так что судить, всё ли чисто, по этому отчёту нельзя.",
  draft_note:
    "черновик не пустой: в нём одно общее основание — сохранённая форма отчёта повреждена.",
  draft: `${UNREAD_COMPOSITION}: сохранённая форма отчёта повреждена`,
};

const GAP_BROKEN_BODY: FirstLayerGap = {
  kind: "broken_body",
  what: "Тело этого отчёта не читается: сохранённые данные повреждены и как данные не разбираются.",
  means:
    "Пунктов замечаний в нём нет не потому, что их не было, а потому, что прочитать их нельзя — " +
    "состав замечаний неизвестен, так что судить, всё ли чисто, по такому отчёту нельзя.",
  draft_note:
    "черновик не пустой: в нём одно общее основание — тело отчёта не читается.",
  draft: `${UNREAD_COMPOSITION}: тело отчёта повреждено и не разбирается`,
};

const GAP_ABSENT: FirstLayerGap = {
  kind: "absent",
  what: "Отчёт сохранён пустым: в нём нет ни человеческого разбора, ни текста.",
  means:
    "Что нашёл аудит, из такого отчёта не узнать — состав замечаний неизвестен, " +
    "так что судить, всё ли чисто, по нему нельзя.",
  draft_note:
    "черновик не пустой: в нём одно общее основание — отчёт сохранён без разбора.",
  draft: `${UNREAD_COMPOSITION}: отчёт сохранён без разбора`,
};

const GAP_NOT_PUBLISHED: FirstLayerGap = {
  kind: "not_published",
  what: "Человеческий разбор к этому отчёту не передан: экран видит только сохранённые подробности.",
  means:
    "Список замечаний по-человечески не собран — состав замечаний неизвестен; " +
    "загляни в подробности ниже, но судить, всё ли чисто, по этому экрану нельзя.",
  draft_note:
    "черновик не пустой: в нём одно общее основание — человеческий разбор отчёта не передан.",
  draft: `${UNREAD_COMPOSITION}: человеческий разбор отчёта не передан`,
};

const GAP_UNKNOWN: FirstLayerGap = {
  kind: "unknown",
  what: "Экран не может сказать, что именно в этом отчёте: формат ответа платформы ему незнаком.",
  means: "Состав замечаний неизвестен — судить, всё ли чисто, по этому отчёту нельзя.",
  draft_note:
    "черновик не пустой: в нём одно общее основание — формат отчёта незнаком экрану.",
  draft: `${UNREAD_COMPOSITION}: формат отчёта экрану незнаком`,
};

/**
 * Why the panel has no human layer for this report, or `null` when it has one
 * (a readable form). The branch comes from the stored `source` token — never
 * from sniffing the text — so each outcome gets its own honest wording
 * (rework R1: an absent layer is not «no findings»).
 */
export function firstLayerGap(report: QualityReportView, body: ReportBody | null): FirstLayerGap | null {
  if (!body) return null;
  if (body.kind === "form") {
    // A readable form whose first layer the platform did not publish.
    return GAP_NOT_PUBLISHED;
  }
  switch (report.source) {
    case "flat_text":
      return GAP_LEGACY;
    case "corrupt_form":
    case "human_form": // the stored shape failed the guard -> damaged form
      return GAP_DAMAGED_FORM;
    case "corrupt_body":
      return GAP_BROKEN_BODY;
    case "absent":
      return GAP_ABSENT;
    default:
      return GAP_UNKNOWN;
  }
}

/** Width of the short artifact label used in the panel (R4: a named
 * constant, not a literal at the slice site). The canonical task-id width
 * lives in Rust (`devai_core::types::ID_PREFIX_CHARS`); an artifact label is
 * the panel's own display choice, so its width is declared here. */
export const ARTIFACT_SHORT_CHARS = 8;

/** The short artifact label used in the damaged-form marker. */
export function shortArtifact(id: string): string {
  return id.length > ARTIFACT_SHORT_CHARS ? id.slice(0, ARTIFACT_SHORT_CHARS) : id;
}

/** The freshness banner of the panel, as one decision: «no banner» or the
 * ready head (the platform's marker, quoted verbatim) plus its fact line.
 * The panel template lays out exactly this — no branch of its own (rework R5:
 * the stale-banner branch is covered by `tests/report_view.test.ts`). */
export type FreshnessBanner =
  | { stale: false }
  | {
      stale: true;
      /** Платформенный маркер — служебный слой; печатается в подробностях. */
      head: string;
      /** Сырые факты стора (статус отчёта, фаза задачи, execution) — тоже в
       * подробностях: в первом слое стоят только обычные слова. */
      detail: string;
      /** Что случилось и что делать — обычными словами, первым делом
       * (REQ-SAY-01 AC7): устаревший отчёт не приглашает решать, он
       * объясняет и говорит, чего ждать. */
      plain: string;
    };

/** Устаревший отчёт: почему решать нельзя, обычными словами. */
const STALE_PLAIN = "Этот отчёт устарел — решать по нему уже нельзя: он больше не текущий.";
/** Что делать, когда новый аудит уже идёт (факт стора: RUNNING execution). */
const STALE_TODO_RUNNING = " Новый аудит уже идёт — дождись его отчёта, тогда решение появится.";
/** Что делать, когда нового прогона ещё нет: ждать новый отчёт. */
const STALE_TODO_WAITING = " Дождись нового отчёта аудита — решение появится вместе с ним.";

/** The human sentence of an outdated report, built ONLY from the stored
 * freshness facts — the running/waiting split is the one fact the store
 * records (`execution_status`), nothing else is inferred. */
export function stalePlain(freshness: ReportFreshness | null | undefined): string {
  const todo =
    freshness?.execution_status === "RUNNING" ? STALE_TODO_RUNNING : STALE_TODO_WAITING;
  return STALE_PLAIN + todo;
}

export function freshnessBanner(
  freshness: ReportFreshness | null | undefined,
  reportStatus: string,
): FreshnessBanner {
  if (!freshness || !freshness.stale) return { stale: false };
  const parts = [`статус отчёта ${reportStatus}`, `фаза ${freshness.task_state}`];
  if (freshness.execution) {
    parts.push(`execution ${shortArtifact(freshness.execution)} (${freshness.execution_status})`);
  }
  return {
    stale: true,
    head: freshness.marker,
    detail: parts.join(" · "),
    plain: stalePlain(freshness),
  };
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

/** The collapsed proof layer exists exactly when evidence was stored. */
export function hasEvidence(it: HumanFormItem): boolean {
  return (it.evidence?.length ?? 0) > 0;
}

// ---------------------------------------------------------------------------
// Importance filter (REQ-QG-04 AC4/AC5/AC6). The filter is a pure VIEW over
// the stored groups: it hides the rows of the groups that were not chosen and
// counts the rows it does show, so each chip carries a number — those counts
// describe the view and are never folded back into the stored form. Grouping,
// order, verdict and wording stay the saved ones, and «всё» restores that
// view untouched. The choice is not sent anywhere and writes nothing.
// ---------------------------------------------------------------------------

/** The filter's «all» token — the stored view as published. */
export const ALL_GROUPS = "all";

/** One filter choice: «всё» plus every saved, non-empty group by its title. */
export interface GroupFilterOption {
  key: string;
  title: string;
  count: number;
}

export function filterOptions(form: HumanForm): GroupFilterOption[] {
  const visible = visibleGroups(form);
  return [
    {
      key: ALL_GROUPS,
      title: "всё",
      count: visible.reduce((n, g) => n + g.items.length, 0),
    },
    ...visible.map((g) => ({ key: g.key, title: g.title, count: g.items.length })),
  ];
}

/**
 * The groups the panel prints under the chosen filter: the selected saved
 * group alone, or every visible group for «всё» / an unknown key. Order is
 * the stored order; nothing is recomputed.
 */
export function groupsForFilter(form: HumanForm, selected: string): HumanFormGroup[] {
  if (!selected || selected === ALL_GROUPS) return visibleGroups(form);
  const chosen = visibleGroups(form).find((g) => g.key === selected);
  return chosen ? [chosen] : visibleGroups(form);
}

/** The site line of one expanded item: an absent site is named in words.
 * ONE definition, shared with the return draft and the signature card
 * (rework R3). */
export { siteText } from "./form_rules.ts";
