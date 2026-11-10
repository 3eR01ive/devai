// Pure rules of the QA gate's return draft and its two hold-actions
// (REQ-QG-02/03/05, rework of 10.11 REQ-SAY-01/05). The component only lays
// these out; here live the decisions a test can drive without a browser:
//
//   * «что от тебя хотят» — ONE line above the two buttons, said from the
//     platform's availability facts; when it offers neither act the line says
//     what the screen is waiting for instead of a dead button (REQ-SAY-01 AC6);
//   * each button is signed by its CONSEQUENCE, in plain words — no internal
//     state token (ACCEPTED / REWORK) anywhere in the first layer;
//   * the draft arrives FILLED with every finding the human can see — decision
//     items and the ones that do not block — so the operator removes what is
//     extra instead of assembling from zero (REQ-SAY-05). «не проверено» is a
//     hole in the data, not a defect: it starts OUTSIDE the draft and is added
//     with one click (ratified OQ 1 of 09.11);
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
import type { FirstLayerRow, QualityGateView } from "./types";
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

/** What «Принять» does, in the words of the button itself (REQ-SAY-01 AC3). */
export const ACCEPT_LABEL = "Принять — работа считается сделанной";

/** What «Вернуть в доработку» does, in the words of the button itself. */
export const RETURN_LABEL =
  "Вернуть в доработку — задача пойдёт на переделку с этими замечаниями";

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

/**
 * The draft the report comes WITH (REQ-SAY-05 AC1/AC3): every finding the
 * operator sees, in the stored human phrase, each with its site. Order is the
 * finding order the panel prints. Nothing is invented: the phrase IS the
 * stored one, and the row stays editable.
 */
export function prefillDraft(findings: FirstLayerRow[]): ReturnDraftItem[] {
  return findings.map((f) => draftItemOf({ id: f.id, summary: f.what, site: f.site }));
}

/** Is this point already in the draft? (The panel shows it as a toggle.) */
export function inDraft(draft: ReturnDraftItem[], id: string): boolean {
  return draft.some((d) => d.item_id === id);
}

/**
 * One click on a point: a finding that is already drafted comes out, a point
 * that is not drafted goes in. «не проверено» is added the same way — the
 * operator must be able to include it, but it is not pre-drafted (ratified
 * OQ 1).
 */
export function toggleDraftItem(
  draft: ReturnDraftItem[],
  item: { id: string; summary: string; site: string },
): ReturnDraftItem[] {
  return inDraft(draft, item.id)
    ? removeDraftItem(draft, item.id)
    : addDraftItem(draft, item);
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
 * its requirement and its site (or the honest «место не указано»). Empty when
 * no point carries a requirement.
 */
export function draftText(draft: ReturnDraftItem[]): string {
  const points = sentPoints(draft);
  if (!points.length) return "";
  const lines = ["Пункты возврата:"];
  points.forEach((p, i) => {
    lines.push(`${i + 1}. ${p.requirement}`);
    lines.push(`   где: ${siteText(p.site)}`);
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
    return rationaleValid(rationale) ? "" : "без обоснования принять нельзя — напиши пару слов почему";
  }
  return composeReturnRationale(rationale, draft).trim() !== ""
    ? ""
    : "без обоснования или пунктов возврат недоступен";
}

/**
 * «Что от тебя хотят» — the ONE line a human reads first, plus the reason a
 * decision is not available when the platform offers neither act
 * (REQ-SAY-01 AC1/AC6). Every branch states what the platform actually says:
 * no act is offered, an audit is still running, or one of the two acts is
 * refused for a named reason. Nothing is invented about a future state.
 */
export interface DecisionPrompt {
  /** The appeal itself — what the person should decide. */
  head: string;
  /** Why there is no decision right now; empty when both acts are open. */
  why: string;
}

export function decisionPrompt(gate: QualityGateView | null): DecisionPrompt {
  if (!gate) {
    return {
      head: "Принять или вернуть эту работу здесь нельзя — платформа не предлагает такого выбора.",
      why: "Отчёт показан для чтения.",
    };
  }
  const accept = !!gate.may_accept;
  const back = !!gate.may_return;
  if (accept && back) {
    return { head: "Твоё решение: принять эту работу или вернуть её на переделку.", why: "" };
  }
  if (accept) {
    return {
      head: "Твоё решение: принять эту работу.",
      why: denied("вернуть", gate.return_denied?.text),
    };
  }
  if (back) {
    return {
      head: "Твоё решение: вернуть эту работу на переделку.",
      why: denied("принять", gate.accept_denied?.text),
    };
  }
  if (gate.audit_running) {
    return {
      head: "Сейчас решать нечего: аудит ещё идёт.",
      why: "Пока проверка не закончилась, принять или вернуть нельзя.",
    };
  }
  return {
    head: "Сейчас ни принять, ни вернуть нельзя.",
    why: denied("принять", gate.accept_denied?.text) || denied("вернуть", gate.return_denied?.text),
  };
}

/** The platform's own refusal, in a sentence — quoted, never paraphrased. */
function denied(act: string, text: string | undefined): string {
  return text ? `${act} нельзя: ${text}` : "";
}
