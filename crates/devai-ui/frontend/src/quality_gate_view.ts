// Pure rules of the QA gate's return draft and its two hold-actions
// (REQ-QG-02/03/05). The component only lays these out; here live the
// decisions a test can drive without a browser:
//
//   * the draft is built from the STORED report items — a requirement is
//     prefilled with the stored summary VERBATIM and stays editable;
//   * a point with an empty requirement never reaches the sent text;
//   * the return is blocked only when the composed text (draft + manual
//     rationale) is empty;
//   * acceptance never requires the draft, only a non-empty rationale.
//
// Nothing here writes, sends or re-groups the report: it is a form helper.
//
// Two operator rules are NOT restated here (rework R3): the stored-site
// wording and the non-empty rationale live once in `form_rules.ts`, consumed
// by the panel, the signature card and this module — so the surfaces can
// never disagree.
import type { QualityGateView } from "./types";
import { rationaleValid, siteText } from "./form_rules.ts";

export { siteText };

/** One editable point of the return draft. */
export interface ReturnDraftItem {
  /** Stable key of the row (the stored item id). */
  key: string;
  item_id: string;
  /** The stored summary, kept for the record (never rewritten). */
  summary: string;
  /** The stored site; empty means the report named none. */
  site: string;
  /** What the operator wants fixed — prefilled verbatim, freely editable. */
  requirement: string;
}

export type GateAct = "accept" | "return";

/** The draft row of one stored item: requirement = summary, byte for byte. */
export function draftItemOf(item: { id: string; summary: string; site: string }): ReturnDraftItem {
  return {
    key: item.id,
    item_id: item.id,
    summary: item.summary,
    site: item.site,
    requirement: item.summary,
  };
}

/** Add one stored point; a second add of the same point is a no-op. */
export function addDraftItem(
  draft: ReturnDraftItem[],
  item: { id: string; summary: string; site: string },
): ReturnDraftItem[] {
  if (draft.some((d) => d.item_id === item.id)) return draft;
  return [...draft, draftItemOf(item)];
}

export function editDraftRequirement(
  draft: ReturnDraftItem[],
  key: string,
  requirement: string,
): ReturnDraftItem[] {
  return draft.map((d) => (d.key === key ? { ...d, requirement } : d));
}

export function removeDraftItem(draft: ReturnDraftItem[], key: string): ReturnDraftItem[] {
  return draft.filter((d) => d.key !== key);
}

/** The points that will actually be sent: an empty requirement is dropped. */
export function sentPoints(draft: ReturnDraftItem[]): { requirement: string; site: string }[] {
  return draft
    .map((d) => ({ requirement: d.requirement.trim(), site: d.site.trim() }))
    .filter((p) => p.requirement !== "");
}

/**
 * The draft as the operator sees it before sending. Numbered, each point with
 * its requirement and its site (or the honest «сайт не указан»). Empty when
 * no point carries a requirement.
 */
export function draftText(draft: ReturnDraftItem[]): string {
  const points = sentPoints(draft);
  if (!points.length) return "";
  const lines = ["Пункты возврата:"];
  points.forEach((p, i) => {
    lines.push(`${i + 1}. ${p.requirement}`);
    lines.push(p.site ? `   сайт: ${p.site}` : `   ${siteText(p.site)}`);
  });
  return lines.join("\n");
}

/**
 * The rationale the return act stores: the operator's own words plus the
 * drafted points. Either half may be empty; the caller blocks the act only
 * when the WHOLE text is empty (REQ-QG-05 AC4).
 */
export function composeReturnRationale(
  rationale: string,
  draft: ReturnDraftItem[],
): string {
  const parts: string[] = [];
  if (rationale.trim()) parts.push(rationale.trim());
  const points = draftText(draft);
  if (points) parts.push(points);
  return parts.join("\n\n");
}

/** Acceptance sends the manual rationale alone — the draft is not required. */
export function acceptRationale(rationale: string): string {
  return rationale.trim();
}

/**
 * Acceptance availability as the WINDOW adds it: the platform's `may_accept`
 * plus a non-empty rationale (REQ-QG-02 AC4). Nothing else.
 */
export function canAccept(gate: QualityGateView | null, rationale: string): boolean {
  return !!gate?.may_accept && rationaleValid(rationale);
}

/**
 * Return availability as the WINDOW adds it: the platform's `may_return` plus
 * a non-empty composed text (REQ-QG-05 AC4). A drafted point alone is enough
 * — the operator may send the points without extra prose.
 */
export function canReturn(
  gate: QualityGateView | null,
  rationale: string,
  draft: ReturnDraftItem[],
): boolean {
  return !!gate?.may_return && composeReturnRationale(rationale, draft).trim() !== "";
}

/**
 * Why a button is off, in words. The platform's refusal is NOT repeated here
 * (it is printed from `gate.*_denied`); this only explains the WINDOW's own
 * condition — the empty rationale (REQ-QG-02 AC2).
 */
export function rationaleHint(act: GateAct, rationale: string, draft: ReturnDraftItem[]): string {
  if (act === "accept") {
    return rationaleValid(rationale) ? "" : "без непустого обоснования приёмка недоступна";
  }
  return composeReturnRationale(rationale, draft).trim() !== ""
    ? ""
    : "без непустого обоснования возврат недоступен";
}

/** What the accept act will do — shown BEFORE the hold (REQ-QG-03 AC1). */
export function acceptPreview(): string {
  return "Принять: задача станет ПРИНЯТОЙ (ACCEPTED). Это ваше решение, автоматики нет.";
}

/** What the return act will do — shown BEFORE the hold (REQ-QG-03 AC2). */
export function returnPreview(draft: ReturnDraftItem[]): string {
  const points = sentPoints(draft);
  if (!points.length) {
    return "Вернуть: задача уйдёт в ДОРАБОТКУ (REWORK); черновика нет — уходит только обоснование.";
  }
  const list = points
    .map((p, i) => `${i + 1}. ${p.requirement} — ${siteText(p.site)}`)
    .join("\n");
  return `Вернуть: задача уйдёт в ДОРАБОТКУ (REWORK) с этими пунктами:\n${list}`;
}
