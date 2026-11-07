// View-model of the homes cycle (REQ-HOME-03/04/06/07) — the pure half of
// the four views that gained frontend behaviour this cycle:
// HomesView (registry rows, recent list, activation affordance),
// TasksView/ProjectsView (attributed blocks, per-home filter, honest
// unavailable branch), TaskDetailView (foreign-home gate) and the
// degraded_from banner in App.vue.
//
// Why it lives here: the SFC templates must not carry untested branching
// logic (R2 quality return — the frontend of this cycle had no tests).
// Every branch they render is a function below, so the node harness in
// `frontend/tests/` executes exactly the decisions the window makes, with
// no browser and no new dependency (`node --test`, type stripping).

import { HOME_CODES } from "./generated/home-codes.ts";
import type {
  HomeIssue,
  HomeRecord,
  HomeRecordsView,
  SessionInfo,
  Task,
} from "./types";

/** Inclusion/state token rendered in the homes registry table. */
export type HomeState = "ACTIVE" | "DISABLED" | "MISSING" | "INCLUDED";

/**
 * One state per registry row, in the operator-visible precedence:
 * active first (the writable home), then switched off, then a home that is
 * included but not on disk. An active row is never rendered as "включён".
 */
export function homeState(
  h: Pick<HomeRecord, "active" | "enabled" | "available">,
): HomeState {
  if (h.active) return "ACTIVE";
  if (!h.enabled) return "DISABLED";
  if (!h.available) return "MISSING";
  return "INCLUDED";
}

/**
 * The activation button is offered only for a home that is included and
 * readable — activating a missing or switched-off home would promise a
 * switch the registry refuses anyway (REQ-HOME-06/07).
 */
export function canActivate(
  h: Pick<HomeRecord, "active" | "enabled" | "available">,
): boolean {
  return !h.active && h.enabled && h.available;
}

/**
 * Machine codes of the homes vocabulary — taken from the GENERATED module
 * (`src/generated/home-codes.ts`, rendered from
 * `crates/devai-api/src/codes.rs` by `scripts/gen-home-codes.mjs`). There is
 * no hand-written copy in TS any more (R1 quality return: a Rust-only code
 * used to reach the UI invisibly; now the dictionary is rendered from the
 * producer and the parity test fails on a stale render).
 */
export { HOME_CODES } from "./generated/home-codes.ts";
export type { HomeCode, HomeCodeName } from "./generated/home-codes.ts";

/**
 * Heading of the recent list, next to the predicate it describes: because
 * `recentHomes` returns EVERY registry record (newest first), the heading
 * must not claim "newly added only" — the two are asserted together in the
 * node harness (R1 quality return).
 */
export const RECENT_HEADING =
  "Недавно добавленные (все дома реестра, свежие первыми):";

/**
 * "Недавно добавленные" = the registry read by `added_at`, newest first —
 * the list the heading promises. It is NOT a second predicate over
 * inclusion: a switched-off home is still a recently added home (it was
 * added, then switched off), it just carries its status in the label
 * (R1 quality return: the old predicate returned disabled records under a
 * heading promising recent ones). Clicking a row re-adds the path
 * (`home.add` re-enables a known record and keeps its original `added_at`,
 * REQ-HOME-03 AC2), so both states are actionable from this list.
 */
export function recentHomes<T extends { added_at: string }>(homes: T[]): T[] {
  return [...homes].sort((a, b) => Date.parse(b.added_at) - Date.parse(a.added_at));
}

/** Chip label of the recent list: the home and its inclusion status. */
export function recentHomeLabel(h: {
  project: string;
  path: string;
  enabled: boolean;
}): string {
  return h.enabled
    ? `${h.project} · ${h.path}`
    : `${h.project} · ${h.path} · выключен`;
}

/** Stored schema rank as the table prints it; unknown stays honest. */
export function schemaLabel(version: number | null): string {
  return version === null ? "—" : `v${version}`;
}

/**
 * Provenance note of a registry row's project label (R3 quality return):
 * "the store was read and has no projects" and "the store could not be
 * read" are different facts and the row states which one it is instead of
 * presenting a path-shape fallback as the home's own name. Null when the
 * label came from the home's own projects (or from an older registry row
 * that did not record the provenance).
 */
export function projectLabelNote(h: {
  project_source: "store" | "path" | "unreadable" | "unknown";
  project_error: string | null;
}): string | null {
  switch (h.project_source) {
    case "unreadable":
      return h.project_error
        ? `Метка выведена по пути: стор не прочитался (${h.project_error})`
        : "Метка выведена по пути: стор не прочитался";
    case "path":
      return "Метка из формы пути: в доме нет проектов";
    default:
      return null;
  }
}

/** Version/read cause of one row, or null when there is nothing to say. */
export function homeIssue(h: {
  version_note: string | null;
  read_error: HomeIssue | null;
}): HomeIssue | null {
  // The version note is the stronger fact about the home's READABILITY as
  // such; a typed read error is the localized cause. The row renders both,
  // so this helper only answers "is there a typed cause at all".
  return h.read_error ?? null;
}

/**
 * The branch a view takes on a typed home issue (R2 quality return,
 * rel/error-context): the wire carries a machine `code` beside the text,
 * so the UI decides by the code instead of probing for "is there a
 * string". `unknown` is a stated fail-closed branch: a code this build does
 * not know is still an unavailable home, never a silent success.
 */
export type HomeIssueKind =
  | "unavailable"
  | "noStore"
  | "schemaVersion"
  | "registryCorrupt"
  | "unknown";

export function homeIssueKind(
  issue: HomeIssue | null | undefined,
): HomeIssueKind | null {
  if (!issue) return null;
  switch (issue.code) {
    case HOME_CODES.unavailable:
      return "unavailable";
    case HOME_CODES.noStore:
      return "noStore";
    case HOME_CODES.schemaVersion:
      return "schemaVersion";
    case HOME_CODES.registryCorrupt:
      return "registryCorrupt";
    default:
      return "unknown";
  }
}

/**
 * The human line of a typed issue: the platform's own message, plus the
 * platform's own hint when it carries one. The UI adds no wording of its
 * own here — a second sentence would be a second truth.
 */
export function homeIssueText(issue: HomeIssue | null | undefined): string | null {
  if (!issue) return null;
  return issue.hint ? `${issue.message} — ${issue.hint}` : issue.message;
}

/** The line of a registry row: version note first, then the typed cause. */
export function homeIssueLine(h: {
  version_note: string | null;
  read_error: HomeIssue | null;
}): string | null {
  return h.version_note ?? homeIssueText(h.read_error);
}

/** The line of a merged-view block: its typed cause, or null. */
export function blockIssueText(b: { error: HomeIssue | null }): string | null {
  return homeIssueText(b.error);
}

/** The code the merged-view banner shows: the typed cause, else the
 * fail-closed unavailable token (a block is only rendered unavailable with
 * a cause in practice; the fallback is stated, not silent). */
export function blockIssueCode(b: { error: HomeIssue | null }): string {
  return b.error?.code ?? HOME_CODES.unavailable;
}

/** Filter options of the merged view: one per enabled home block. */
export function homeFilterOptions<T>(
  blocks: HomeRecordsView<T>[],
): { value: string; label: string }[] {
  return blocks.map((b) => ({ value: b.home_path, label: b.home_project }));
}

/** Rows of one attributed block of the (already filtered) merged view. */
export function rowsFor<T>(blocks: HomeRecordsView<T>[], homePath: string): T[] {
  return blocks.find((b) => b.home_path === homePath)?.records ?? [];
}

/**
 * Project scoping inside the merged view (TasksView `?project=`): rows are
 * filtered per block, never merged across homes — same-named tasks of two
 * homes stay in their own blocks (REQ-HOME-04 AC3).
 */
export function tasksForProject<T extends { project_id: string | null }>(
  blocks: HomeRecordsView<T>[],
  projectFilter: string | null,
): HomeRecordsView<T>[] {
  if (!projectFilter) return blocks;
  return blocks.map((b) => ({
    ...b,
    records: b.records.filter((t) => t.project_id === projectFilter),
  }));
}

/** Deep link of a task row: the home travels in the query (gate input). */
export function taskDetailLink(
  block: Pick<HomeRecordsView<Task>, "home_path">,
  task: Pick<Task, "id">,
): string {
  return `/task/${task.id}?home=${encodeURIComponent(block.home_path)}`;
}

/** Attribution printed on a row of the merged view. */
export function homeAttribution(
  block: Pick<HomeRecordsView<unknown>, "home_project" | "is_active">,
): string {
  return block.is_active
    ? `${block.home_project} · активный`
    : block.home_project;
}

/** Empty-state wording of a block: an empty project ≠ an empty home. */
export function tasksEmptyText(projectFilter: string | null): string {
  return projectFilter
    ? "У проекта в этом доме пока нет задач"
    : "В доме нет задач — пустое состояние, не ошибка";
}

/**
 * Foreign-home gate (REQ-HOME-04 AC3, ratified OQ-HOME-2/5): task details
 * are read from the ACTIVE home only. A deep link carrying another home is
 * not followed and not silently switched — the view offers an explicit
 * activation instead. The gate compares the same string the backend
 * reports as `session.home`; no path normalization is invented in the UI.
 */
export function foreignHome(
  routeHome: string | null,
  activeHome: string | null,
): string | null {
  if (!routeHome) return null;
  if (activeHome === null) return null;
  return routeHome === activeHome ? null : routeHome;
}

/**
 * Status-bar banner of a degraded startup (REQ-HOME-07): the stored active
 * home could not be opened, the window observes through the control home.
 * Null when the open was clean. Wording lives here so the tested string is
 * the rendered one.
 *
 * Ratified OQ-HOME-8 adds one derived state to the same channel: no active
 * home at all (every record switched off) — the window must say that
 * plainly and point at the two way out actions, never pretend a home is
 * active and never pick one silently.
 */
export function degradedBanner(session: SessionInfo | null): string | null {
  const d = session?.degraded_from;
  if (!d) return null;
  if (d.code === HOME_CODES.noneActive) {
    return (
      "Нет активного дома: доменные записи недоступны, пока вы не включите " +
      "или не добавите дом. Реестр хранит все записи — включите дом или " +
      "добавьте новый на вкладке «Дома»."
    );
  }
  return (
    `Сохранённый активный дом ${d.home} не открылся (${d.message}) — ` +
    "окно наблюдает через контрольный дом; хранимая запись активного дома " +
    "не изменена. Верните путь на диск или активируйте другой дом на " +
    "вкладке «Дома»."
  );
}

/**
 * The registry view's own fact (ratified OQ-HOME-8): with at least one
 * record and none of them active, the window states it and offers the two
 * ways out (enable a record / add a home) instead of leaving a silent
 * "nothing is active" gap. Null while the registry is empty or a home is
 * active.
 */
export function noActiveHomeNote(homes: Pick<HomeRecord, "active">[]): string | null {
  if (homes.length === 0) return null;
  if (homes.some((h) => h.active)) return null;
  return (
    "Нет активного дома — все записи выключены. Включите дом кнопкой " +
    "«Включить» или добавьте новый: включённый вами дом станет активным."
  );
}
