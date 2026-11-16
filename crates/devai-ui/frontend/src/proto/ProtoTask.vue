<script setup lang="ts">
// Новая карточка задачи (по макету): вертикальный путь, один открытый шаг,
// «вход/выход» у ролей, «качество» и «правила приёмки» у гейтов; расход
// токенов по шагам, вклад каждого пункта приёмки и вставки кода.
import { computed, nextTick, onMounted, onUnmounted, ref, watch, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useHoldToConfirm } from "../components/signature/useHoldToConfirm";
import { api, toUiError } from "../api";
import type {
  ArtifactView, ProgressView, QualityReportView, Question, RulebookView,
  SignatureView, TaskUsageView, UiError, WorkflowEvent,
  WorkspaceChangesView, WorkspaceDiffView,
} from "../types";
import type { ExecRow, ExecTranscriptView, LiveDiffView, LiveWorktreeView } from "../types";
import CodeRefText from "./CodeRefText.vue";
import SignPanel from "./SignPanel.vue";
import QuestionsBlock from "./QuestionsBlock.vue";
import { parseStatementReview, type StatementReview } from "./statementReview";
import StatementVerdict from "./StatementVerdict.vue";
import RunConsole from "./RunConsole.vue";
import "./proto.css";

const route = useRoute();
const router = useRouter();
const id = computed(() => String(route.params.id ?? ""));

const prog = ref<ProgressView | null>(null);
const report = ref<QualityReportView | null>(null);
const rules = ref<RulebookView | null>(null);
const usage = ref<TaskUsageView | null>(null);
// Документ, который подписывают: спека из того же чтения, что карточка подписи.
const signature = ref<SignatureView | null>(null);
// Изменения задачи — ФАКТ от платформы (ветка, база, tip, файлы), а не слова
// исполнителя. Diff каждого файла читается лениво: список не тянет мегабайты.
const changes = ref<WorkspaceChangesView | null>(null);
const diffByPath = ref<Record<string, { text: string | null; binary: boolean } | "loading">>({});
// Разложенный по строкам diff: подряд идущий неизменённый контекст схлопнут
// в провал «⋯ N строк». Развернуть можно провал или весь файл целиком —
// diff приходит с полным контекстом файла, догружать нечего.
type DiffItem = { kind: "line"; cls: string; text: string } | { kind: "hidden"; n: number; run: string[] };
const itemsByPath = ref<Record<string, DiffItem[]>>({});
const openRuns = ref<Record<string, boolean>>({});
const openFile = ref<string | null>(null);
// Отчёт исполнителя — его собственные слова о работе. Человек читает их как
// рассказ, а не как доказательство: доказательство — во вкладке worktree.
const implWords = ref<{ paragraph: string | null; notes: string | null } | null>(null);
// Артефакты задачи: по ним вкладки шага показывают данные ВЫБРАННОГО прогона,
// а не всегда последнего. Тела читаются лениво и кешируются.
const artifacts = ref<ArtifactView[]>([]);
const selRunId = ref<Record<string, string>>({});
const runBodies = ref<Record<string, any>>({});
// Сравнение с ПРЕДЫДУЩЕЙ версией: галочка в шаге анализа, без отдельной вкладки.
const comparePrev = ref<Record<string, boolean>>({});
const error = ref<UiError | null>(null);
const busy = ref(false);

const WORD: Record<string, string> = {
  STATEMENT: "постановка", ANALYSIS: "анализ", REVIEW: "ревью",
  APPROVAL: "подпись", IMPLEMENTATION: "реализация", QA: "приёмка",
};
const STATE_WORD: Record<string, string> = {
  DRAFT: "черновик", STATEMENT: "у постановщика", ANALYSIS: "анализ", REVIEW: "ревью",
  APPROVAL: "ждёт подписи", IMPLEMENTATION: "код", QA: "приёмка", REWORK: "доработка",
  WAITING_FOR_INPUT: "ждёт человека", PAUSED: "на паузе", BLOCKED: "остановлена",
  FAILED: "упала", ACCEPTED: "принята", CANCELLED: "отменена", REJECTED: "отклонена",
};

const body = computed<Record<string, any>>(
  () => (report.value?.report as Record<string, any>) ?? {},
);
// Отчёт качества ВЫБРАННОГО прогона: когда он выбран, приёмка считает по нему.
const qaRaw = ref<any | null>(null);
const qaBody = computed<Record<string, any>>(() => qaRaw.value ?? body.value);
// Проверки нормализуем один раз: у непроверенного пункта балла может не быть
// вовсе (undefined), а не только null — иначе сумма потерь даёт NaN.
const checks = computed<any[]>(() =>
  (qaBody.value.evaluation?.checks ?? []).map((c: any) => ({
    ...c,
    score: typeof c.score === "number" ? c.score : null,
    weight: typeof c.weight === "number" ? c.weight : 0,
  })),
);
const headline = computed<Record<string, any>>(() => qaBody.value.human_view?.headline ?? {});
const score = computed<number | null>(
  () => (typeof headline.value.score === "number" ? headline.value.score : null),
);
const theta = computed<number | null>(
  () => (typeof headline.value.theta === "number" ? headline.value.theta : null),
);
const verdict = computed<string>(() => String(headline.value.verdict ?? ""));
// Коммит, на котором читается код по ссылкам из отчётов. Приоритет — тот,
// что платформа проштамповала в отчёте реализации (это и есть коммит задачи);
// запасной — голова из отчёта качества.
const implCommit = ref<string>("");
// Ревизия для чтения кода: коммит задачи, иначе голова из отчёта качества,
// иначе коммит, от которого задача началась (база воркспейса). Последнее
// важно для ссылок, сделанных до реализации: кода задачи ещё нет, а база —
// то состояние, которое автор видел. Совсем без ревизии — HEAD репозитория
// (это решает платформа, когда названного коммита уже нет).
const rev = computed<string>(
  () =>
    implCommit.value ||
    String(qaBody.value.workspace_revision?.git_head ?? "") ||
    String(changes.value?.bound_head ?? changes.value?.head_before ?? ""),
);

const scoreTone = computed(() => {
  if (score.value === null) return "wait";
  if (theta.value !== null && score.value < theta.value) return "bad";
  return verdict.value && !verdict.value.includes("НЕ ГОТОВ") ? "ok" : "wait";
});

const tokensByExec = computed(() => {
  const m = new Map<string, number>();
  for (const r of usage.value?.rows ?? []) m.set(r.execution, r.total);
  return m;
});
const costByExec = computed(() => {
  const m = new Map<string, number>();
  for (const r of usage.value?.rows ?? []) m.set(r.execution, r.cost);
  return m;
});
/** Сходимость стадии из прогресса: у ревью — объём, у приёмки — балл. */
function convOf(state: string) {
  const c: any = (prog.value as any)?.convergence;
  const key = state === "REVIEW" ? "spec_review" : state === "QA" ? "quality" : null;
  if (!key || !c?.stages) return null;
  return (c.stages as any[]).find((x) => x.stage === key) ?? null;
}
function convSpark(state: string): { cls: string; h: number }[] {
  const st = convOf(state);
  if (!st?.series?.length) return [];
  const isSpec = st.stage === "spec_review";
  const tail = st.series.slice(-22);
  const vals = st.series.map((r: any) => (isSpec ? r.requirements_count : r.score_milli)).filter((v: any) => v != null) as number[];
  const max = vals.length ? Math.max(...vals) : 1;
  const theta = (st.series.find((r: any) => r.theta_milli != null) ?? {}).theta_milli ?? null;
  return tail.map((r: any) => {
    const v = isSpec ? r.requirements_count : r.score_milli;
    if (v == null) return { cls: "un", h: 4 };
    if (isSpec) return { cls: "mid", h: 4 + Math.round((v / (max || 1)) * 10) };
    return { cls: theta != null && v < theta ? "bad" : "ok", h: 4 + Math.round((v / 1000) * 10) };
  });
}
function convMode(state: string): string {
  const st = convOf(state);
  const last = st?.series?.[st.series.length - 1];
  const map: Record<string, string> = { CONVERGING: "рост", DIVERGING: "расхождение", PLATEAU: "застой", STUCK: "стоит", UNKNOWN: "нет данных" };
  return last ? map[last.mode] ?? "нет данных" : "";
}

function tok(n: number | undefined) {
  if (!n) return "";
  if (n >= 1e9) return (n / 1e9).toFixed(2) + " млрд";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " млн";
  if (n >= 1e3) return Math.round(n / 1e3) + " тыс";
  return String(n);
}

function age(iso: string) {
  const s = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 1000));
  if (s < 60) return `${s} с`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} мин`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h} ч ${m % 60} мин` : `${Math.floor(h / 24)} д ${h % 24} ч`;
}

// ── прогоны и их артефакты ──────────────────────────────────────────────
const artByExec = computed(() => {
  const m = new Map<string, ArtifactView>();
  for (const a of artifacts.value) {
    if (a.execution_id) m.set(a.execution_id, a); // последний по порядку побеждает
  }
  return m;
});

/** Выбранный прогон шага; по умолчанию — ПОСЛЕДНИЙ. */
function runOf(s: any): any | null {
  const execs = s?.execs ?? [];
  if (!execs.length) return null;
  const want = selRunId.value[s.state];
  return (want ? execs.find((e: any) => e.id === want) : null) ?? execs[execs.length - 1];
}

/** Артефакт выбранного прогона шага (null — прогон ничего не отдал). */
function artOf(s: any): ArtifactView | null {
  const e = runOf(s);
  return e ? artByExec.value.get(e.id) ?? null : null;
}

async function pickRun(s: any, execId: string) {
  selRunId.value = { ...selRunId.value, [s.state]: execId };
  const art = artByExec.value.get(execId);
  if (!art) { if (s.state === "QA") qaRaw.value = null; return; }
  await  await ensureBody(art);
  void loadRunRow(s);
  // Тело могло быть УЖЕ прочитано (кэш), и тогда ensureBody ничего не делает —
  // раньше из-за этого приёмка продолжала показывать прошлый отчёт: засечка
  // переключалась, а число нет. Привязываем отчёт к ВЫБРАННОМУ прогону явно.
  if (art.kind === "QUALITY_REPORT") {
    qaRaw.value = (runBodies.value as Record<string, any>)[art.id] ?? null;
  }
}

async function ensureBody(art: ArtifactView) {
  if (runBodies.value[art.id]) return;
  try {
    const c = await api.artifactContent(art.id);
    if (c.shape !== "text") return;
    const parsed = JSON.parse(c.text);
    runBodies.value = { ...(runBodies.value as Record<string, any>), [art.id]: parsed };
    if (art.kind === "QUALITY_REPORT") qaRaw.value = parsed;
  } catch {
    /* тело не прочиталось — вкладка честно скажет, что данных нет */
  }
}

/** Тело артефакта выбранного прогона (или null). */
function bodyOf(s: any): any | null {
  const art = artOf(s);
  if (!art) return null;
  const b = runBodies.value as Record<string, any>;
  return b[art.id] ?? null;
}

/** Спецификация выбранного прогона; если прогонов нет — документ из подписи. */
function specBody(s: any): any | null {
  if ((s.execs ?? []).length) {
    const b = bodyOf(s);
    const art = artOf(s);
    return art?.kind === "REQUIREMENTS_SPEC" && b ? b : null;
  }
  return signature.value?.spec ?? null;
}

/** Отчёт проверки выбранного прогона (или документ из подписи у шага без прогонов). */
function reviewBody(s: any): any | null {
  if ((s.execs ?? []).length) {
    const b = bodyOf(s);
    const art = artOf(s);
    return art?.kind === "REVIEW_REPORT" && b ? b : null;
  }
  return signature.value?.review ?? null;
}

/** Отчёт реализации выбранного прогона (branch/commit/абзац). */
function implBody(s: any): any | null {
  if ((s.execs ?? []).length) {
    const b = bodyOf(s);
    const art = artOf(s);
    return art?.kind === "IMPLEMENTATION_REPORT" && b ? b : null;
  }
  return implWords.value;
}

// ── сравнение версий спеки ──────────────────────────────────────────────
/** Предыдущий прогон шага, который отдал артефакт нужного вида. */
function prevRunWithArt(s: any, kind: string): { run: any; art: ArtifactView } | null {
  const execs: any[] = s.execs ?? [];
  const cur = runOf(s);
  if (!cur) return null;
  const idx = execs.findIndex((e) => e.id === cur.id);
  for (let i = idx - 1; i >= 0; i -= 1) {
    const a = artByExec.value.get(execs[i].id);
    if (a && a.kind === kind) return { run: execs[i], art: a };
  }
  return null;
}

function bodyOfArt(artId: string): any | null {
  const b = runBodies.value as Record<string, any>;
  return b[artId] ?? null;
}

/** Построчный diff: одинаковые строки — контекст, разные — с минусом и плюсом. */
type DiffLine = { t: "same" | "add" | "del"; v: string };
function diffLines(a: string, b: string): DiffLine[] {
  const A = (a || "").split("\n");
  const B = (b || "").split("\n");
  // LCS-таблица: строк в абзацах немного, квадрат допустим.
  const n = A.length;
  const m = B.length;
  const L: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      L[i][j] = A[i] === B[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) { out.push({ t: "same", v: A[i] }); i += 1; j += 1; }
    else if (L[i + 1][j] >= L[i][j + 1]) { out.push({ t: "del", v: A[i] }); i += 1; }
    else { out.push({ t: "add", v: B[j] }); j += 1; }
  }
  while (i < n) { out.push({ t: "del", v: A[i] }); i += 1; }
  while (j < m) { out.push({ t: "add", v: B[j] }); j += 1; }
  return out;
}

type ReqDiff = {
  id: string;
  kind: "added" | "removed" | "changed" | "same";
  statement?: DiffLine[];
  ac?: { t: "add" | "del"; v: string }[];
  oldStatement?: string;
  newStatement?: string;
};

/** Что изменилось в спеке выбранного прогона против предыдущей версии. */
function specCompare(s: any): { prevSeq: number | null; items: ReqDiff[] } | null {
  const art = artOf(s);
  const cur = bodyOf(s);
  if (!art || art.kind !== "REQUIREMENTS_SPEC" || !cur) return null;
  const prev = prevRunWithArt(s, "REQUIREMENTS_SPEC");
  if (!prev) return null;
  const old = bodyOfArt(prev.art.id);
  if (!old) return null;

  const oldReqs: any[] = old.requirements ?? [];
  const newReqs: any[] = cur.requirements ?? [];
  const ids = Array.from(new Set([...oldReqs.map((r) => r.id), ...newReqs.map((r) => r.id)]));
  const items: ReqDiff[] = [];
  for (const id of ids) {
    const o = oldReqs.find((r) => r.id === id);
    const nw = newReqs.find((r) => r.id === id);
    if (!o) { items.push({ id, kind: "added", newStatement: nw?.statement ?? "" }); continue; }
    if (!nw) { items.push({ id, kind: "removed", oldStatement: o.statement ?? "" }); continue; }
    const os = String(o.statement ?? "");
    const ns = String(nw.statement ?? "");
    const oldAc: string[] = o.acceptance_criteria ?? [];
    const newAc: string[] = nw.acceptance_criteria ?? [];
    const acDiff = [
      ...oldAc.filter((x) => !newAc.includes(x)).map((v) => ({ t: "del" as const, v })),
      ...newAc.filter((x) => !oldAc.includes(x)).map((v) => ({ t: "add" as const, v })),
    ];
    const changed = os !== ns || acDiff.length > 0;
    items.push({
      id,
      kind: changed ? "changed" : "same",
      statement: changed && os !== ns ? diffLines(os, ns) : undefined,
      oldStatement: os,
      newStatement: ns,
      ac: acDiff,
    });
  }
  return { prevSeq: prev.art.seq, items };
}

type FindDiff = { t: "add" | "del" | "same"; severity?: string; text: string; req?: string };

/** Что изменилось в проверке выбранного прогона против предыдущей проверки. */
function reviewCompare(s: any): {
  prevSeq: number | null;
  verdictOld: string;
  verdictNew: string;
  verdictChanged: boolean;
  paragraph: DiffLine[] | null;
  findings: FindDiff[];
} | null {
  const art = artOf(s);
  const cur = bodyOf(s);
  if (!art || art.kind !== "REVIEW_REPORT" || !cur) return null;
  const prev = prevRunWithArt(s, "REVIEW_REPORT");
  if (!prev) return null;
  const old = bodyOfArt(prev.art.id);
  if (!old) return null;

  const key = (f: any) => String(f.description ?? "").trim();
  const oldF: any[] = old.findings ?? [];
  const newF: any[] = cur.findings ?? [];
  const findings: FindDiff[] = [];
  for (const f of newF) {
    const hit = oldF.find((o) => key(o) === key(f));
    findings.push({
      t: hit ? "same" : "add",
      severity: f.severity,
      text: key(f),
      req: f.requirement_ref,
    });
  }
  for (const o of oldF) {
    if (!newF.some((f) => key(f) === key(o))) {
      findings.push({ t: "del", severity: o.severity, text: key(o), req: o.requirement_ref });
    }
  }
  const op = String(old.human_paragraph ?? "");
  const np = String(cur.human_paragraph ?? "");
  return {
    prevSeq: prev.art.seq,
    verdictOld: String(old.verdict ?? ""),
    verdictNew: String(cur.verdict ?? ""),
    verdictChanged: String(old.verdict ?? "") !== String(cur.verdict ?? ""),
    paragraph: op !== np ? diffLines(op, np) : null,
    findings,
  };
}

/** Догружаем тело артефакта, когда шаг раскрыт: вкладкам нужны данные. */
async function toggleCompare(s: any) {
  const on = !comparePrev.value[s.state];
  comparePrev.value = { ...comparePrev.value, [s.state]: on };
  if (on) {
    const kind = s.state === "REVIEW" ? "REVIEW_REPORT" : "REQUIREMENTS_SPEC";
    const prev = prevRunWithArt(s, kind);
    if (prev) await ensureBody(prev.art);
  }
}

function ensureLoaded(s: any) {
  const art = artOf(s);
  if (art) void ensureBody(art);
  void loadRunRow(s);
}

// ── строка прогона: почему он встал ──────────────────────────────────────
// Отчёт роли НЕ всегда есть: прогон может упасть, и тогда «ничего не отдал» —
// правда без причины. Причина записана платформой (exec.get): статус, класс
// отказа, текст отказа. Показываем её, а не пустоту.
const runRow = ref<ExecRow | null>(null);
/** Поток упавшего прогона: платформа не всегда пишет текст отказа в строку
 *  (например, прогон прибит извне), но поток роли на диске есть — читаем его. */
const runTail = ref<string>("");

async function loadRunRow(s: any) {
  const e = runOf(s);
  if (!e) { runRow.value = null; runTail.value = ""; return; }
  if (runRow.value?.id === e.id) return;
  try {
    runRow.value = await api.execGet(e.id);
  } catch {
    runRow.value = null;
  }
  runTail.value = "";
  // Поток читаем только у НЕ дошедшего до отчёта прогона: у живого и у
  // успешного за отказом идти незачем.
  const broken = ["FAILED", "CANCELLED"].includes(String(runRow.value?.status ?? ""));
  if (broken && !runRow.value?.error) {
    try {
      const t = await api.execTranscript(id.value, e.id);
      runTail.value = [t.stdout, t.stderr].filter((x) => x && x.trim()).join("\n— stderr —\n");
    } catch {
      runTail.value = "";
    }
  }
}

/** Прогон шага упал — это состояние ШАГА, а не всей задачи: у отказа своя
 *  вкладка внутри шага, и баннеров поверх карточки он не заводит. */
function isFailed(s: any): boolean {
  const e = runOf(s);
  if (!e) return false;
  const st = String(e.status);
  return st === "FAILED" || st === "CANCELLED";
}

/** Слово для метки шага: прогон упал или его остановили — это разные факты. */
function failChipWord(s: any): string {
  const e = runOf(s);
  return e && String(e.status) === "CANCELLED" ? "остановлен" : "упал";
}

/** Класс отказа человеческими словами (платформа называет его машинно). */
const FAIL_WORD: Record<string, string> = {
  provider: "провайдер",
  provider_rate: "провайдер ограничил темп или квоту",
  provider_quota: "квота провайдера исчерпана",
  form: "ответ роли не прошёл форму",
  platform: "ошибка платформы",
};
const failClassWord = computed(() => {
  const c = runRow.value?.failure_class ?? "";
  return FAIL_WORD[c] ?? c;
});


/// Выводы проверяющего ДЛЯ ВЫБРАННОГО прогона шага: вкладка обязана меняться
/// от прогона к прогону, а не показывать всегда самый свежий вердикт.
function reviewOf(s: any): StatementReview | null {
  const art = artOf(s);
  const body = art ? (runBodies.value as Record<string, any>)[art.id] : null;
  return parseStatementReview(body) ?? (art ? null : stmtReview.value);
}

function durationOf(execs: any[]): string {
  let ms = 0;
  for (const e of execs) {
    if (!e.started_at) continue;
    const a = new Date(e.started_at).getTime();
    const b = e.finished_at ? new Date(e.finished_at).getTime() : Date.now();
    ms += Math.max(0, b - a);
  }
  if (!ms) return "";
  const m = Math.round(ms / 60000);
  return m < 60 ? `${m} мин` : `${Math.floor(m / 60)} ч ${m % 60} мин`;
}

// ── шаги ────────────────────────────────────────────────────────────────
// В графе бывает ДВА узла на состояние: роль и гейт (QA: аудитор + подпись).
// Ключ «только состояние» склеивал их и терял прогоны роли — держим оба.
const roleNodes = computed(
  () => new Map((prog.value?.graph.nodes ?? []).filter((n) => n.kind !== "gate").map((n) => [n.state, n])),
);
const gateNodes = computed(
  () => new Map((prog.value?.graph.nodes ?? []).filter((n) => n.kind === "gate").map((n) => [n.state, n])),
);
const nodes = computed(() => new Map((prog.value?.graph.nodes ?? []).map((n) => [n.state, n])));

const steps = computed(() => {
  const g = prog.value?.graph;
  if (!g) return [];
  const allowed = prog.value?.allowed_operations ?? [];
  return g.stages.map((s) => {
    // прогоны и роль берём у УЗЛА РОЛИ: у гейта прогонов нет вовсе.
    // Чип «решает человек» живёт на этапе (s.gate), а не на узле.
    const n = roleNodes.value.get(s.state) ?? gateNodes.value.get(s.state);
    const execs = n?.executions ?? [];
    const running = execs.filter((e) => String(e.status) === "RUNNING");
    const tokens = execs.reduce((acc, e) => acc + (tokensByExec.value.get(e.id) ?? 0), 0);
    const runOp = n?.role_slug ? allowed.find((o) => o === `run-role-execution(${n.role_slug})`) : "";
    const isQa = s.gate && s.state === "QA" && !!report.value;
    return {
      state: s.state,
      title: WORD[s.state] ?? s.label,
      label: s.label,
      gate: s.gate,
      status: s.status,
      word: s.status_word,
      slug: n?.role_slug ?? null,
      version: n?.role_version ?? null,
      inContract: n?.input_contract ?? null,
      outContract: n?.output_contract ?? null,
      execs,
      running: running.length > 0,
      runningId: running[0]?.id ?? null,
      canRun: !s.gate && (!!runOp || (!!n?.role_slug && allowed.includes("run"))),
      runOp,
      why: s.gate ? "решает человек" : s.status === "passed" ? "шаг пройден" : "платформа пока не разрешает запуск",
      tokens,
      cost: execs.reduce((acc, e) => acc + (costByExec.value.get(e.id) ?? 0), 0),
      duration: durationOf(execs),
      limit: (prog.value?.attempt_limit as any)?.role === n?.role_slug
        ? (prog.value?.attempt_limit as any)?.limit ?? null : null,
      score: isQa ? score.value : null,
    };
  });
});

const openState = ref<string | null>(null);
const openTab = ref<string>("in");

watch(steps, (list) => {
  if (openState.value && list.some((s) => s.state === openState.value)) return;
  const cur = list.find((s) => s.status === "current") ?? list[list.length - 1];
  openState.value = cur?.state ?? null;
  openTab.value = cur?.state === "QA" ? "qa" : cur?.state === "APPROVAL" ? "sign" : "in";
}, { immediate: true });

async function loadImplReport() {
  if (implWords.value) return;
  try {
    const arts = await api.artifactsList(id.value);
    const reports = arts
      .filter((a) => a.kind === "IMPLEMENTATION_REPORT")
      .sort((a, b) => b.seq - a.seq);
    if (!reports.length) return;
    const c = await api.artifactContent(reports[0].id);
    if (c.shape !== "text") return;
    const body = JSON.parse(c.text) as { human_paragraph?: string; notes?: string };
    implWords.value = { paragraph: body.human_paragraph ?? null, notes: body.notes ?? null };
  } catch {
    implWords.value = null;
  }
}

async function loadChanges() {
  if (changes.value) return;
  try {
    changes.value = await api.taskWorkspaceChanges(id.value);
  } catch {
    changes.value = null;
  }
}

async function toggleFile(path: string) {
  openFile.value = openFile.value === path ? null : path;
  if (openFile.value !== path) return;
  if (diffByPath.value[path] && diffByPath.value[path] !== "loading") return;
  diffByPath.value = { ...diffByPath.value, [path]: "loading" };
  try {
    // полный контекст файла: из него окно и разворачивает скрытые строки
    const d: WorkspaceDiffView = await api.taskWorkspaceDiffFull(id.value, path);
    diffByPath.value = { ...diffByPath.value, [path]: { text: d.diff, binary: d.binary } };
    itemsByPath.value = { ...itemsByPath.value, [path]: diffItems(d.diff) };
  } catch {
    diffByPath.value = { ...diffByPath.value, [path]: { text: null, binary: false } };
  }
}

// Классификация строк unified diff: шапка, кусок, добавлено, удалено, контекст.
function clsOf(l: string): string {
  if (l.startsWith("+++") || l.startsWith("---")) return "meta";
  if (l.startsWith("@@")) return "hunk";
  if (l.startsWith("+")) return "add";
  if (l.startsWith("-")) return "del";
  if (l.startsWith("diff --git") || l.startsWith("index ") || l.startsWith("new file") ||
      l.startsWith("deleted file") || l.startsWith("similarity index") ||
      l.startsWith("rename from") || l.startsWith("rename to")) return "meta";
  return "ctx";
}

// Провал оставляем только у длинных кусков контекста: по три строки сверху и
// снизу видно, остальное — «⋯ N строк». Порог 12 — чтобы не прятать мелочь.
function diffItems(text: string | null): DiffItem[] {
  if (!text) return [];
  const lines = text.split("\n");
  const out: DiffItem[] = [];
  let i = 0;
  while (i < lines.length) {
    if (clsOf(lines[i]) !== "ctx") {
      out.push({ kind: "line", cls: clsOf(lines[i]), text: lines[i] });
      i += 1;
      continue;
    }
    let k = i;
    while (k < lines.length && clsOf(lines[k]) === "ctx") k += 1;
    const run = lines.slice(i, k);
    if (run.length > 12) {
      run.slice(0, 3).forEach((t) => out.push({ kind: "line", cls: "ctx", text: t }));
      out.push({ kind: "hidden", n: run.length - 6, run: run.slice(3, run.length - 3) });
      run.slice(run.length - 3).forEach((t) => out.push({ kind: "line", cls: "ctx", text: t }));
    } else {
      run.forEach((t) => out.push({ kind: "line", cls: "ctx", text: t }));
    }
    i = k;
  }
  return out;
}

const runKey = (path: string, i: number) => `${path}#${i}`;
const isOpen = (path: string, i: number) => !!openRuns.value[runKey(path, i)];
const foldsOf = (path: string) => (itemsByPath.value[path] ?? []).filter((it) => it.kind === "hidden").length;

// Плоский список строк для <pre>: внутри pre любой перенос из разметки
// становится пустой строкой, поэтому рендер — ОДИН span на строку.
function shown(path: string): { cls: string; text: string; foldKey?: number }[] {
  const out: { cls: string; text: string; foldKey?: number }[] = [];
  (itemsByPath.value[path] ?? []).forEach((it, k) => {
    if (it.kind === "line") {
      out.push({ cls: it.cls, text: it.text });
    } else if (isOpen(path, k)) {
      it.run.forEach((t) => out.push({ cls: "ctx", text: t }));
    } else {
      out.push({ cls: "fold", text: `⋯ ${it.n} строк скрыто · развернуть`, foldKey: k });
    }
  });
  return out;
}

function unfoldRun(path: string, i: number) {
  openRuns.value = { ...openRuns.value, [runKey(path, i)]: true };
}
function foldAll(path: string, open: boolean) {
  const next = { ...openRuns.value };
  (itemsByPath.value[path] ?? []).forEach((it, i) => {
    if (it.kind === "hidden") next[runKey(path, i)] = open;
  });
  openRuns.value = next;
}

const tabsFor = (s: any) => {
  // Постановка: сам текст и отдельной вкладкой — выводы проверяющего
  // (вердикт, чек-лист, его вопросы), а не только текст.
  const execs0: any[] = s.execs ?? [];
  if (s.state === "STATEMENT") {
    const t: [string, string][] = [
      ["stmt", "постановка"],
      ["verdict", "оценка проверяющего"],
    ];
    if (isFailed(s)) t.unshift(["fail", "отказ"]);
    if (execs0.length) t.push(["term", "консоль"]);
    return t;
  }
  if (s.state === "APPROVAL") return [["sign", "что подписываешь"]] as [string, string][];

  const execs = execs0;
  const art = artOf(s);
  // Прогоны есть, но выбранный ничего не отдал — вкладок нет: подсовывать
  // данные соседнего прогона было бы враньём.
  // Прогон ничего не отдал: либо он упал (тогда у отказа своя вкладка), либо
  // просто не оставил данных — во втором случае остаётся только консоль.
  if (execs.length && !art) {
    // У идущего прогона отчёта ещё нет — но именно тогда и нужны консоль и
    // ЖИВОЕ дерево: правки роли видны, пока она работает.
    const t: [string, string][] = [];
    if (isFailed(s)) t.push(["fail", "отказ"]);
    t.push(["term", "консоль"]);
    if (s.state === "IMPLEMENTATION") t.push(["wt", "worktree"]);
    return t;
  }

  const t: [string, string][] = [];
  if (art?.kind === "REQUIREMENTS_SPEC") t.push(["spec", "спецификация"]);
  if (art?.kind === "REVIEW_REPORT") t.push(["review", "проверка"]);
  if (art?.kind === "IMPLEMENTATION_REPORT") t.push(["impl", "реализация"]);
  if (s.state === "IMPLEMENTATION") t.push(["wt", "worktree"]);
  if (art?.kind === "QUALITY_REPORT") {
    t.push(["qa", "качество"]);
    t.push(["rules", "правила приёмки"]);
  }
  // Шаг без прогонов (гейт или ещё не запускался) — по документам из подписи.
  if (!execs.length) {
    if (s.state === "ANALYSIS" && signature.value?.spec) t.push(["spec", "спецификация"]);
    if (s.state === "REVIEW" && signature.value?.review) t.push(["review", "проверка"]);
    if (s.state === "QA" && report.value) {
      t.push(["qa", "качество"]);
      t.push(["rules", "правила приёмки"]);
    }
  }
  if (isFailed(s)) t.unshift(["fail", "отказ"]);
  // Консоль есть у любого шага с прогонами: человек видит, что агент делает
  // ПРЯМО СЕЙЧАС, а не догадывается по артефактам.
  if (execs.length) t.push(["term", "консоль"]);
  return t;
};

// Открытая вкладка обязана существовать в СВОЁМ шаге: если выбран другой
// прогон (или шаг открылся сам), и прежней вкладки там нет — встаём на первую.
// Без этого шаг показывал пустую панель: вкладки есть, ничего не выбрано.
watchEffect(() => {
  const s = steps.value.find((x: any) => x.state === openState.value);
  if (!s) return;
  const keys = tabsFor(s).map((t) => t[0]);
  if (!keys.includes(openTab.value)) openTab.value = keys[0] ?? "";
});

function openStep(s: any) {
  if (openState.value === s.state) { openState.value = null; return; }
  openState.value = s.state;
  if (s.state === "IMPLEMENTATION") {
    void loadChanges();
    void loadImplReport();
  }
  ensureLoaded(s);
  const t = tabsFor(s);
  stopTerm();
  term.value = null;
  openTab.value = t.length ? t[0][0] : "";
}

// ── приёмка ─────────────────────────────────────────────────────────────
const mode = ref<"loss" | "all">("loss");
const picked = ref<Set<string>>(new Set());
const openCheck = ref<string | null>(null);

const loss = (c: any) => (c.score === null ? -1 : (1 - c.score) * (c.weight ?? 0));
const contrib = (c: any): number | null => (c.score === null ? null : c.score * (c.weight ?? 0));
const contribText = (c: any): string => {
  const v = contrib(c);
  return v === null ? "—" : v.toFixed(3);
};

const rows = computed(() => {
  let list = checks.value.slice();
  if (mode.value === "loss") {
    list = list.filter((c) => c.score === null || c.score < 1);
    list.sort((a, b) => loss(b) - loss(a) || (b.weight ?? 0) - (a.weight ?? 0));
  } else {
    list.sort((a, b) => (b.weight ?? 0) - (a.weight ?? 0) || String(a.id).localeCompare(String(b.id)));
  }
  return list;
});
const maxWeight = computed(() => rows.value.reduce((m, c) => Math.max(m, c.weight ?? 0), 0) || 1);
const totalWeight = computed(() => checks.value.reduce((s, c) => s + (c.weight ?? 0), 0));
const scoredWeight = computed(() => checks.value.reduce((s, c) => s + (c.score === null ? 0 : (c.weight ?? 0)), 0));
const contribSum = computed(() => checks.value.reduce((s, c) => s + (c.score ?? 0) * (c.weight ?? 0), 0));
const lossSum = computed(() => checks.value.reduce((s, c) => s + (c.score === null ? 0 : (1 - c.score) * (c.weight ?? 0)), 0));
const losses = computed(() => checks.value.filter((c) => c.score === null || c.score < 1).length);

// По умолчанию «на доработку» отмечено то, что НЕ дотягивает до 1.0 (и то,
// что вовсе не оценено): пункт с полным баллом чинить нечего, и держать его в
// списке возврата — шум, из-за которого причина обрастает лишними строками.
// Слово оператора: всё ниже 1.0 правится без исключений, «якорей» нет.
watch(checks, (list) => {
  if (!list.length || picked.value.size > 0) return;
  const weak = list.filter((c: any) => c.score === null || c.score < 1);
  picked.value = new Set(weak.map((c: any) => c.id));
}, { immediate: true });

function togglePick(c: any, ev: Event) {
  ev.stopPropagation();
  const next = new Set(picked.value);
  next.has(c.id) ? next.delete(c.id) : next.add(c.id);
  picked.value = next;
}

const ruleFor = (cid: string) => {
  for (const g of rules.value?.groups ?? []) {
    const hit = g.items.find((i) => i.id === cid);
    if (hit) return { group: g.group, ...hit };
  }
  return null;
};

const REF = /([\w./-]+\.(?:rs|ts|vue|js|py|yaml|yml|json|toml|sh|sql))(?::(\d+)(?:-(\d+))?)?/g;

/** Текст проверки, разложенный на куски: проза и вставки кода на месте ссылок. */
type Piece =
  | { t: "text"; v: string }
  | { t: "code"; path: string; from: number; to: number };

// Любое поле, которое должно быть текстом, печатаем текстом: объект с
// формулировкой берём по смыслу, всё прочее — JSON, но никогда «[object Object]».
function txt(v: any): string {
  if (v == null) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  if (Array.isArray(v)) return v.map(txt).join(" ");
  if (typeof v === "object") {
    for (const k of ["text", "description", "statement", "claim", "value", "v"]) {
      if (typeof v[k] === "string") return v[k];
    }
    try { return JSON.stringify(v); } catch { return String(v); }
  }
  return String(v);
}

function evidenceOf(c: any): string {
  const e = c.violations?.[0] ?? c.evidence?.[0] ?? "";
  return txt(e).slice(0, 1200);
}

// ── влитие: факт и ход ────────────────────────────────────────────────────
// Слияние не стадия процесса (задача от него не двигается), поэтому это НЕ шаг
// графа, а факт на карточке принятой задачи: либо «влито» с коммитом, либо
// кнопка влития. Акт тот же, что `devai task merge` — окно запускает
// платформенную команду, а не свою реализацию git.
const mergeDecision = computed<any | null>(() => {
  const rows = decisions.value.filter((d: any) => d.kind === "rework_merge");
  return rows.length ? rows[rows.length - 1] : null;
});
const merged = computed(() => !!mergeDecision.value);
const mergeBusy = ref(false);
const mergeErr = ref<string | null>(null);
const mergeDone = ref<{ branch: string; commit: string } | null>(null);

const mergeInfo = computed(() => {
  const d = mergeDecision.value;
  if (!d) return null;
  return {
    branch: (d.refs ?? [])[0] ?? "",
    commit: (d.refs ?? [])[1] ?? "",
    actor: d.actor ?? "",
    at: d.created_at ?? "",
    rationale: d.rationale ?? "",
  };
});

async function doMerge() {
  mergeBusy.value = true;
  mergeErr.value = null;
  try {
    const out = await api.taskMerge(id.value, "влито человеком из карточки задачи");
    mergeDone.value = { branch: out.branch, commit: out.commit };
    await load();
  } catch (e) {
    mergeErr.value = toUiError(e).message;
  } finally {
    mergeBusy.value = false;
  }
}

const { holding: mergeHolding, progress: mergeProgress, start: mergeStart, move: mergeMove, release: mergeRelease } =
  useHoldToConfirm(
    () => !merged.value && !mergeBusy.value,
    () => void doMerge(),
  );

// ── где мы СЕЙЧАС ─────────────────────────────────────────────────────────
// Платформа помечает стадию «current», только пока по ней что-то идёт. Стоит
// задача (BLOCKED/PAUSED/FAILED, ждёт ответа) — и метка исчезает: все узлы
// зелёные, и на втором круге не понять, где именно ты стоишь. Поэтому «сейчас»
// считаем сами: идущий прогон → его стадия; стоящее дело → стадия, на которую
// оно вернётся; иначе само состояние (стадия или гейт).
const nowState = computed<string | null>(() => {
  const p = prog.value as any;
  if (!p) return null;
  const run = (p.running_executions ?? [])[0];
  if (run?.state) return String(run.state);
  const t = p.task ?? {};
  const held = ["WAITING_FOR_INPUT", "BLOCKED", "PAUSED", "FAILED"].includes(String(t.state));
  if (held) return t.return_state ? String(t.return_state) : String(t.state);
  return String(t.state);
});
const isNow = (s: any) => s.state === nowState.value;

/** Слово на метке: дело идёт, стоит или ждёт человека — это разные «сейчас». */
const heldWord = computed(() => {
  const st = String((prog.value as any)?.task?.state ?? "");
  if ((prog.value as any)?.running_executions?.length) return "идёт";
  if (st === "WAITING_FOR_INPUT") return "ждёт ответа";
  if (["BLOCKED", "PAUSED", "FAILED"].includes(st)) return "стоит здесь";
  if (st === "APPROVAL") return "ждёт подписи";
  if (st === "QA") return "ждёт приёмки";
  return "сейчас";
});

// ── живое рабочее дерево: что роль правит прямо сейчас ────────────────────
// Отчёты и диффы показывают ЗАКОММИЧЕННОЕ (baseline..tip). Пока роль работает,
// её правки ещё не в коммите — и окно показывало пустой worktree у идущего
// прогона. Здесь фактическое состояние каталога задачи: файлы, строки, ветка.
const live = ref<LiveWorktreeView | null>(null);
const liveErr = ref<string | null>(null);
let liveTimer: number | null = null;

async function liveOnce() {
  try {
    live.value = await api.workspaceLive(id.value);
    liveErr.value = null;
  } catch (e) {
    liveErr.value = toUiError(e).message;
  }
}

/** Открыть файл живого дерева: незакоммиченный дифф как он есть. */
const liveOpen = ref<string | null>(null);
const liveDiff = ref<LiveDiffView | null>(null);

async function openLiveFile(path: string) {
  if (liveOpen.value === path) { liveOpen.value = null; liveDiff.value = null; return; }
  liveOpen.value = path;
  liveDiff.value = null;
  try {
    liveDiff.value = await api.workspaceLiveDiff(id.value, path);
  } catch (e) {
    liveDiff.value = { path, diff: "", note: toUiError(e).message };
  }
}

function startLive() {
  if (liveTimer !== null) return;
  void liveOnce();
  liveTimer = window.setInterval(() => void liveOnce(), 2000);
}

function stopLive() {
  if (liveTimer !== null) { window.clearInterval(liveTimer); liveTimer = null; }
}

// ── консоль прогона: тот же поток, что пишет роль ─────────────────────────
// Читаем улики прогона (`role_stdout.txt`/`role_stderr.txt`) хвостом: пока роль
// работает, поток растёт и окно перечитывает его раз в секунду. Ничего не
// пересказываем и не буферизуем «своё» — на экране ровно то, что на диске.
const term = ref<ExecTranscriptView | null>(null);
const termErr = ref<string | null>(null);
const termFollow = ref(true);
const termEl = ref<HTMLElement | null>(null);
let termTimer: number | null = null;
let termExec = "";

const termText = computed(() => {
  const t = term.value;
  if (!t) return "";
  return t.stderr ? `${t.stdout}\n— stderr —\n${t.stderr}` : t.stdout;
});

function termStep(): any | null {
  return steps.value.find((s: any) => s.state === openState.value) ?? null;
}

async function termOnce() {
  const s = termStep();
  if (!s) return;
  const e = runOf(s);
  if (!e) { term.value = null; return; }
  if (e.id !== termExec) { term.value = null; termExec = e.id; }
  try {
    const t = await api.execTranscript(id.value, e.id);
    term.value = t;
    termErr.value = null;
    if (termFollow.value) {
      await nextTick();
      const el = termEl.value;
      if (el) el.scrollTop = el.scrollHeight;
    }
    // Прогон закончился — досматривать нечего: опрос прекращается сам, а не
    // молотит улики вечно.
    if (!t.running) stopTerm();
  } catch (err) {
    termErr.value = toUiError(err).message;
    stopTerm();
  }
}

function startTerm() {
  if (termTimer !== null) return;
  void termOnce();
  termTimer = window.setInterval(() => void termOnce(), 1200);
}

function stopTerm() {
  if (termTimer !== null) { window.clearInterval(termTimer); termTimer = null; }
}

/** Смена вкладки включает и выключает наблюдение: консоль живёт, только пока
 *  она открыта. */
function selectTab(_s: any, key: string) {
  openTab.value = key;
  if (key === "term") startTerm();
  else stopTerm();
  if (key === "wt") startLive();
  else stopLive();
}

// ── подпись спеки: два акта гейта ─────────────────────────────────────────
// Разбор вопросов и сборка решения живут в SignPanel/signText; карточка только
// отправляет акт платформы и перечитывает себя.
const signBusy = ref(false);
/** Почему подпись не прочиталась — показываем причину, а не «подписывать нечего». */
const sigError = ref<string | null>(null);

// ── вопросы платформы: слово человека снимает удержание ──────────────────
// Вопрос и гейт берутся из ДАННЫХ: текст вопроса — из чтения вопросов, гейт —
// из события `question.raised` по этому question_id. Гадать по формулировке
// нельзя: слово в тексте — это текст, а не адрес перехода.
const questions = ref<Question[]>([]);
/** Решения человека: из них читается, влита ли задача (rework_merge). */
const decisions = ref<any[]>([]);
// Протокол проверки постановки: чек-лист и вопросы — поля контракта, поэтому
// блок вопросов показывает их по местам, а не склейкой текста.
const stmtReview = ref<StatementReview | null>(null);
const events = ref<WorkflowEvent[]>([]);
const answerBusy = ref<string | null>(null);

const openQs = computed(() => questions.value.filter((q) => q.status === "OPEN"));

async function doAnswer(q: Question, text: string) {
  const value = text.trim();
  if (!value) return;
  answerBusy.value = q.id;
  error.value = null;
  try {
    await api.questionAnswer(id.value, q.id, value);
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    answerBusy.value = null;
  }
}

async function doSign(approve: boolean, rationale: string | null) {
  signBusy.value = true;
  error.value = null;
  try {
    signature.value = approve
      ? await api.taskApprove(id.value, rationale)
      : await api.taskReject(id.value, rationale);
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    signBusy.value = false;
  }
}

// ── данные ──────────────────────────────────────────────────────────────
async function load() {
  error.value = null;
  try {
    const [p, r, u, rb, sig, arts, qs, evs, decs] = await Promise.all([
      api.taskProgress(id.value),
      api.taskReport(id.value).catch(() => null),
      api.taskUsage(id.value).catch(() => null),
      api.evaluationRules().catch(() => null),
      api.taskSignature(id.value).catch((e) => {
        // Отказ чтения подписи — это REASON, а не пустота: иначе карточка
        // врёт «спеки нет», хотя спека есть, а сломался отбор (прецедент
        // 01a1df87: неизвестный вид решения ронял весь отбор подписи).
        sigError.value = String(toUiError(e).message);
        return null;
      }),
      api.artifactsList(id.value).catch(() => [] as ArtifactView[]),
      api.questionsList(id.value).catch(() => [] as Question[]),
      api.taskEvents(id.value, 200).catch(() => [] as WorkflowEvent[]),
      api.taskDecisions(id.value).catch(() => [] as any[]),
    ]);
    prog.value = p;
    if (sig) sigError.value = null;
    report.value = r;
    usage.value = u;
    rules.value = rb;
    signature.value = sig;
    artifacts.value = arts as ArtifactView[];
    questions.value = qs as Question[];
    events.value = evs as WorkflowEvent[];
    decisions.value = decs as any[];
    // Текущий шаг открыт по умолчанию — его строку прогона читаем сразу, иначе
    // причина отказа появлялась бы только после клика по шагу.
    // Задача может стоять ВНЕ стадии (BLOCKED/PAUSED): тогда берём шаг с
    // самыми свежими прогонами — именно там и лежит причина остановки.
    const cur =
      steps.value.find((x: any) => x.state === (prog.value as any)?.task?.state) ??
      steps.value.filter((x: any) => (x.execs ?? []).length).pop();
    if (cur) {
      void loadRunRow(cur);
      // Открытый по умолчанию шаг должен открыться на СВОЁМ первом действии:
      // упавший прогон — на «отказе», а не на пустой панели.
      if (!openTab.value) {
        const t = tabsFor(cur);
        openTab.value = t.length ? t[0][0] : "";
      }
    }
    // Отчёт реализации (последний) называет коммит задачи — по нему и читаем код.
    const impl = (arts as ArtifactView[])
      .filter((a) => a.kind === "IMPLEMENTATION_REPORT")
      .sort((a, b) => b.seq - a.seq)[0];
    if (impl) {
      await ensureBody(impl);
      const b = (runBodies.value as Record<string, any>)[impl.id];
      if (b?.commit) implCommit.value = String(b.commit);
    }
    const stmt = (arts as ArtifactView[])
      .filter((a) => a.kind === "STATEMENT_REVIEW")
      .sort((a, b) => b.seq - a.seq)[0];
    if (stmt) {
      await ensureBody(stmt);
      stmtReview.value = parseStatementReview((runBodies.value as Record<string, any>)[stmt.id]);
    }
  } catch (e) {
    error.value = toUiError(e);
  }
}

async function doRun() {
  busy.value = true;
  try { await api.taskRun(id.value, null, false); await load(); }
  catch (e) { error.value = toUiError(e); }
  finally { busy.value = false; }
}
// ── закрытие дела человеком и возврат к списку ───────────────────────────
// Отмена — удержание самой кнопки, без модального подтверждения: одиночный и
// двойной клик ничего не применяют, мутация идёт тем же актом, что
// `devai task cancel` (причина и автор остаются в истории).
const CLOSED = new Set(["ACCEPTED", "CANCELLED", "REJECTED"]);
const canCancel = computed(() => !!prog.value && !CLOSED.has(prog.value.task.state));
const cancelBusy = ref(false);

async function doCancel() {
  cancelBusy.value = true;
  error.value = null;
  try {
    await api.taskCancel(id.value, "закрыто человеком из карточки задачи");
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    cancelBusy.value = false;
  }
}

// Вернуть в работу можно отменённое, приостановленное, заблокированное и
// упавшее дело: платформа помнит стадию, на которой его остановили.
const RESUMABLE = new Set(["CANCELLED", "PAUSED", "BLOCKED", "FAILED"]);
const canResume = computed(() => !!prog.value && RESUMABLE.has(prog.value.task.state));
const resumeBusy = ref(false);

async function doResume() {
  resumeBusy.value = true;
  error.value = null;
  try {
    await api.taskResume(id.value);
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    resumeBusy.value = false;
  }
}

const { holding, progress, start, move, release } = useHoldToConfirm(
  () => canCancel.value && !cancelBusy.value,
  () => void doCancel(),
);

// ── приёмка: два хода гейта — принять или вернуть на доработку ────────────
// Оба — удержанием на самой кнопке (решение человека, а не случайный клик), и
// живут они ВНУТРИ шага приёмки, рядом с тем, что человек видит.
const qaBusy = ref<"accept" | "return" | null>(null);
const qaErr = ref<string | null>(null);
/** Словами, что чинить: уходят в причину возврата вместе с выбранными пунктами. */
const returnNote = ref("");

const qaOpen = computed(() => prog.value?.task.state === "QA");

const { holding: qaHolding, progress: qaProgress, start: qaStart, move: qaMove, release: qaRelease } =
  useHoldToConfirm(
    () => qaOpen.value && qaBusy.value === null,
    (kind) => {
      if (kind === "accept") void doAccept();
      if (kind === "return") void doReturn();
    },
  );

async function doAccept() {
  qaBusy.value = "accept";
  qaErr.value = null;
  try {
    await api.taskAccept(id.value, null);
    await load();
  } catch (e) {
    qaErr.value = toUiError(e).message;
  } finally {
    qaBusy.value = null;
  }
}

/// Возврат на доработку: причина — слова человека плюс ПУНКТЫ, которые он
/// оставил «на доработку» в таблице. Пустая причина не притворяется пустой
/// фразой: платформа получит перечень пунктов.
async function doReturn() {
  qaBusy.value = "return";
  qaErr.value = null;
  const names = checks.value
    .filter((c: any) => picked.value.has(c.id))
    .map((c: any) => c.id)
    .join(", ");
  const parts = [returnNote.value.trim(), names ? `пункты: ${names}` : ""].filter(Boolean);
  try {
    await api.taskReturn(id.value, parts.length ? parts.join("\n") : null);
    returnNote.value = "";
    await load();
  } catch (e) {
    qaErr.value = toUiError(e).message;
  } finally {
    qaBusy.value = null;
  }
}

// ── перепройти шаг и переписать постановку ───────────────────────────────
// Оба хода живут ТАМ, ГДЕ ОНИ НУЖНЫ: перезапуск — кнопкой в самом шаге,
// новая редакция постановки — правкой текста в шаге постановки. Отдельной
// «панели ошибок» нет: интерфейс не спрашивает «вы ошиблись?», а даёт действие
// на том шаге, где ошибка видна.
const replayBusy = ref<string | null>(null);
// Постановка по умолчанию — ЧТЕНИЕ, как и было; правка включается кнопкой.
const editOpen = ref(false);
const editTitle = ref("");
const editBody = ref("");

// Правка постановки начинается с той, что есть: человек правит текст, а не
// пишет заново в пустом поле.
watch(
  () => prog.value?.task,
  (t) => {
    if (!t) return;
    editTitle.value = t.title;
    editBody.value = t.business_description;
  },
  { immediate: true },
);

const CLOSED_STATES = new Set(["ACCEPTED", "CANCELLED", "REJECTED"]);
// Поставленная, но не начатая задача: единственный ход — начать работу.
const isDraft = computed(() => prog.value?.task.state === "DRAFT");
const startBusy = ref(false);

async function doStart() {
  startBusy.value = true;
  error.value = null;
  try {
    // Два акта платформы подряд и без самодеятельности: DRAFT → STATEMENT
    // (`task start`), затем порождение роли постановщика (`task run`).
    await api.taskStart(id.value);
    await api.taskRun(id.value, null, false);
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    startBusy.value = false;
  }
}
const REWINDABLE = new Set(["STATEMENT", "ANALYSIS", "REVIEW", "APPROVAL", "IMPLEMENTATION", "QA"]);
const currentState = computed(() => prog.value?.task.state ?? "");
const canReplay = (state: string) =>
  REWINDABLE.has(state) && state !== currentState.value && !CLOSED_STATES.has(currentState.value);

async function replay(s: { state: string; title: string }) {
  if (!canReplay(s.state)) return;
  replayBusy.value = s.state;
  error.value = null;
  try {
    await api.taskRewind(
      id.value,
      s.state,
      `шаг «${s.title}» перезапущен человеком — пройти этот шаг заново`,
    );
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    replayBusy.value = null;
  }
}

function startEdit() {
  const t = prog.value?.task;
  if (!t) return;
  editTitle.value = t.title;
  editBody.value = t.business_description;
  editOpen.value = true;
}
function cancelEdit() {
  editOpen.value = false;
}

const editChanged = computed(() => {
  const t = prog.value?.task;
  if (!t) return false;
  return editTitle.value.trim() !== t.title.trim() || editBody.value !== t.business_description;
});
const canAcceptEdit = computed(
  () => editChanged.value && editTitle.value.trim().length > 0 && !CLOSED_STATES.has(currentState.value),
);

async function acceptEdit() {
  if (!canAcceptEdit.value) return;
  busy.value = true;
  error.value = null;
  try {
    await api.taskRestate(
      id.value,
      editTitle.value.trim(),
      editBody.value,
      "постановка переписана человеком: новая редакция принята из карточки",
    );
    editOpen.value = false;
    await load();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    busy.value = false;
  }
}

function backToList() {
  void router.push("/proto/tasks");
}

async function doStop(execId: string) {
  busy.value = true;
  try {
    await api.execCancel(execId, "остановлено человеком из карточки задачи");
    await load();
  }
  catch (e) { error.value = toUiError(e); }
  finally { busy.value = false; }
}

// ── живое обновление карточки ────────────────────────────────────────────
// Карточка не должна требовать «выйти в список и вернуться»: пока она открыта,
// один дешёвый опрос спрашивает ход задачи, и ТОЛЬКО при изменении (стадия,
// время изменения, число живых прогонов) перечитывает всё остальное. Так
// завершившийся прогон видно сам, а IPC не молотит полный набор чтений вхолостую.
let pollTimer: number | undefined;
let pollBusy = false;

async function pollOnce() {
  if (pollBusy || document.hidden) return;
  pollBusy = true;
  try {
    const p = await api.taskProgress(id.value);
    const changed =
      !prog.value ||
      p.task.updated_at !== prog.value.task.updated_at ||
      p.task.state !== prog.value.task.state ||
      (p.running_executions?.length ?? 0) !== (prog.value.running_executions?.length ?? 0);
    if (changed) {
      prog.value = p;
      await load();
    }
  } catch {
    /* следующий тик попробует снова: карточка не должна падать из-за опроса */
  } finally {
    pollBusy = false;
  }
}

// Для тестов: сам опрос без ожидания интервала.
defineExpose({ pollOnce });

function onFocus() {
  void pollOnce();
}

onMounted(() => {
  pollTimer = window.setInterval(() => void pollOnce(), 4000);
  window.addEventListener("focus", onFocus);
});
onUnmounted(() => {
  stopTerm();
  stopLive();
  if (pollTimer) window.clearInterval(pollTimer);
  window.removeEventListener("focus", onFocus);
});

onMounted(load);
</script>

<template>
  <section class="p-root">
    <div v-if="error" class="p-err">{{ error.code }} — {{ error.message }}</div>
    <div v-else-if="!prog" class="p-empty">читаю задачу…</div>

    <template v-else>
      <!-- возврат и закрытие дела: обе кнопки — про навигацию и слово
           человека, а не про шаги роли -->
      <div class="p-dnav">
        <button class="p-back" @click="backToList">← к списку</button>
        <div class="p-dright">
        <button v-if="isDraft" class="p-start" :disabled="startBusy" @click="doStart"
                title="начать работу: DRAFT → постановщик">
          {{ startBusy ? 'запускаю…' : '▶ Начать работу' }}
        </button>
        <button
          v-if="canResume"
          class="p-resume"
          :disabled="resumeBusy"
          :title="'вернуть в работу: дело продолжится с той стадии, где его остановили'"
          @click="doResume"
        >{{ resumeBusy ? 'возвращаю…' : '↩ Вернуть в работу' }}</button>
        <button
          v-else
          class="p-cancel"
          :class="{ holding: holding === 'cancel', done: !canCancel }"
          :disabled="!canCancel || cancelBusy"
          :title="canCancel ? 'удерживай, чтобы отменить задачу' : 'задача уже закрыта'"
          @pointerdown="start('cancel', $event)"
          @pointermove="move($event)"
          @pointerup="release"
          @pointerleave="release"
          @pointercancel="release"
        >
          <i class="fill" :style="{ width: (holding === 'cancel' ? progress * 100 : 0) + '%' }" />
          <span>{{ cancelBusy ? 'отменяю…' : canCancel ? 'Отменить задачу' : 'закрыта' }}</span>
        </button>
        </div>
      </div>

      <header class="p-dhead">
        <div class="p-dtitle">
          <h1>{{ prog.task.display }}</h1>
          <div class="p-ident">
            <span>{{ id.slice(0, 8) }}</span>
            <span>обновлена {{ age(prog.task.updated_at) }} назад</span>
            <span>{{ steps.length }} шагов</span>
          </div>
        </div>
        <div class="p-dside">
          <span class="p-state" :class="prog.task.state === 'QA' ? 'b-pass' : prog.task.state === 'APPROVAL' ? 'b-human' : 'b-now'">
            {{ STATE_WORD[prog.task.state] ?? prog.task.state }}
          </span>
          <span class="p-runchip" :class="prog.running_executions.length ? 'run' : 'warn'">
            <i />{{ prog.running_executions.length ? "идёт" : "прогона нет" }}
          </span>
        </div>
        <div class="p-score" :class="scoreTone">
          <div class="n">{{ score === null ? "—" : (score * 100).toFixed(1) }}<span>%</span></div>
          <div class="l">качество</div>
          <div class="s">{{ verdict ? verdict.toLowerCase() : "оценки нет" }}</div>
        </div>
        <div class="p-tokens" v-if="usage && usage.measured">
          <div class="n">{{ tok(usage.total) }}</div>
          <div class="l">токенов</div>
          <div class="s">${{ usage.cost.toFixed(2) }} · {{ usage.measured }} из {{ usage.runs }} прогонов</div>
        </div>
      </header>

      <QuestionsBlock
        :questions="questions"
        :events="events"
        :busy="answerBusy"
        :protocol="stmtReview"
        @answer="doAnswer"
      />

      <div class="p-steps">
        <div
          v-for="(s, i) in steps"
          :key="s.state"
          class="p-vstep"
          :class="[s.status, { gate: s.gate, open: openState === s.state, now: isNow(s) }]"
        >
          <div class="p-nodebox">
            <div class="p-vnode">{{ s.status === "passed" ? "✓" : "" }}</div>
            <div v-if="i < steps.length - 1" class="rail" />
          </div>
          <div class="p-vcard">
            <div class="p-vhead" @click="openStep(s)">
              <div class="t1">
                <span class="role">{{ s.title }}</span>
                <span v-if="isNow(s)" class="p-nowchip">{{ heldWord }}</span>
                <span v-if="convSpark(s.state).length" class="p-spark" :title="'сходимость: ' + convMode(s.state)">
                  <i v-for="(b, k) in convSpark(s.state)" :key="k" :class="b.cls" :style="{ height: b.h + 'px' }" />
                </span>
                <span v-if="isFailed(s)" class="p-failchip" title="выбранный прогон шага не дошёл до отчёта — причина во вкладке «отказ»">{{ failChipWord(s) }}</span>
                <span v-if="s.score !== null" class="p-scorechip" :class="scoreTone">
                  {{ (s.score * 100).toFixed(1) }}%<i>качество</i>
                </span>
                <span v-if="s.gate" class="p-human" title="решает человек: шаг ждёт подписи или ответа, а не прогона">
                  <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
                    <circle cx="8" cy="4.4" r="2.6" fill="currentColor" />
                    <path d="M8 8.2c-2.9 0-5 1.7-5 4.2V15h10v-2.6c0-2.5-2.1-4.2-5-4.2Z" fill="currentColor" />
                  </svg>
                </span>
              </div>
              <div class="t2">
                <span class="p-mstats">
                  <span v-if="s.duration">{{ s.duration }}</span>
                  <span v-if="s.execs.length">прогонов <b>{{ s.execs.length }}</b></span>
                  <span v-if="s.tokens"><b>{{ tok(s.tokens) }}</b> ток</span>
                  <span v-if="s.cost"><b>${{ s.cost.toFixed(2) }}</b></span>
                  <span v-if="s.limit" class="lim">лимит {{ s.limit }}</span>
                </span>
                <button v-if="s.running" class="p-act stop" :disabled="busy" @click.stop="doStop(s.runningId!)">■ остановить</button>
                <button v-else-if="s.canRun" class="p-act run" :disabled="busy" :title="s.runOp" @click.stop="doRun()">▶ запустить</button>
                <!-- перепройти этот шаг: своё действие у каждого шага, без
                     отдельной панели «вы ошиблись?» -->
                <button
                  v-if="canReplay(s.state)"
                  class="p-act replay"
                  :disabled="replayBusy !== null"
                  :title="'пройти путь заново с шага «' + s.title + '»: результаты этого шага и следующих станут несвежими, история останется'"
                  @click.stop="replay(s)"
                >{{ replayBusy === s.state ? '…' : '↻ отсюда' }}</button>
                <span v-else-if="s.status === 'current'" class="p-why">{{ s.why }}</span>
                <span class="caret">▶</span>
              </div>
            </div>

            <div v-if="openState === s.state" class="p-vbody">
              <!-- прогоны шага: засечка = прогон, по умолчанию выбран последний -->
              <div v-if="s.execs.length" class="p-runs">
                <span class="p-rlabel">прогоны</span>
                <button
                  v-for="(e, k) in s.execs"
                  :key="e.id"
                  class="p-rdhit"
                  :title="'прогон ' + (k + 1) + ' · ' + e.status"
                  @click.stop="pickRun(s, e.id)"
                ><i
                  class="p-rd"
                  :class="[String(e.status) === 'COMPLETED' ? 'ok' : String(e.status) === 'FAILED' ? 'bad' : 'run',
                           runOf(s)?.id === e.id ? 'sel' : '']"
                /></button>
                <span class="p-rsel">выбран {{ s.execs.findIndex((e: any) => e.id === runOf(s)?.id) + 1 }} из {{ s.execs.length }}</span>
              </div>
              <div v-if="s.execs.length" class="p-runmemo" :class="{ bad: isFailed(s) && !artOf(s) }">
                <template v-if="artOf(s)">вкладки ниже — данные этого прогона · {{ artOf(s)!.display }}</template>
                <template v-else-if="isFailed(s)">
                  прогон упал{{ failClassWord ? " · " + failClassWord : "" }} — отчёта нет, причина во вкладке «отказ»
                </template>
                <template v-else>этот прогон ничего не отдал — вкладок нет</template>
              </div>

              <!-- отказ прогона: своя вкладка шага, а не баннер поверх задачи -->
              <div v-if="openTab === 'fail'" class="p-panel">
                <div v-if="!runRow" class="p-empty" style="padding:4px 0">строка прогона не прочиталась</div>
                <template v-else>
                  <div class="rf-h">
                    <span class="rf-dot" />
                    <b>{{ runRow.role_slug }}</b>
                    <span class="rf-cls">{{ failClassWord || "отказ" }}</span>
                    <span v-if="runRow.status" class="rf-st">{{ runRow.status }}</span>
                    <span v-if="runRow.attempt && runRow.attempt > 1" class="rf-att">попытка {{ runRow.attempt }}</span>
                  </div>
                  <div v-if="runRow.error" class="rf-t">{{ runRow.error }}</div>
                  <div v-else-if="runRow.summary" class="rf-t">{{ runRow.summary }}</div>
                  <template v-else-if="runTail">
                    <div class="rf-note">
                      строку отказа платформа не заполнила — ниже поток прогона, как он записан на диске
                    </div>
                    <div class="rf-t">{{ runTail }}</div>
                  </template>
                  <div v-else class="p-empty" style="padding:4px 0">
                    ни строки отказа, ни потока: прогон не оставил ни того, ни другого
                  </div>
                  <div class="rf-acts">
                    <button class="p-act" @click="selectTab(s, 'term')">открыть консоль прогона</button>
                    <button v-if="canResume" class="p-act run" :disabled="resumeBusy" @click="doResume">
                      {{ resumeBusy ? "возвращаю…" : "↩ Вернуть в работу" }}
                    </button>
                  </div>
                </template>
              </div>

              <!-- постановка: исходный текст задачи, без служебных вкладок -->
              <div v-if="tabsFor(s).length" class="p-tabs">
                <button
                  v-for="t in tabsFor(s)"
                  :key="t[0]"
                  class="p-tab"
                  :class="{ on: openTab === t[0] }"
                  @click="selectTab(s, t[0])"
                >{{ t[1] }}</button>
              </div>

              <!-- консоль прогона: терминал как он есть -->
              <div v-if="openTab === 'term'" class="p-panel">
                <div v-if="termErr" class="p-empty" style="color:#ffa972">
                  консоль не прочиталась: {{ termErr }}
                </div>
                <template v-else-if="term">
                  <div class="p-termhead">
                    <span class="tdot" :class="term.running ? 'run' : 'done'"></span>
                    <b>{{ term.role }}</b>
                    <span class="mono">{{ term.status }}</span>
                    <span class="mono">{{ term.stdout_bytes }} Б</span>
                    <span v-if="term.note" class="tnote">{{ term.note }}</span>
                    <button class="p-act" :class="{ on: termFollow }" @click="termFollow = !termFollow">
                      {{ termFollow ? "следовать" : "не следовать" }}
                    </button>
                  </div>
                  <div ref="termEl" class="p-term"><RunConsole :text="termText" /></div>
                </template>
                <div v-else class="p-empty" style="padding:4px 0">у этого прогона нет консоли</div>
              </div>

              <div v-if="s.state === 'STATEMENT' && openTab === 'verdict'" class="p-panel">
                <StatementVerdict :review="reviewOf(s)" />
              </div>

              <div v-if="s.state === 'STATEMENT' && openTab === 'stmt'" class="p-panel">
                <div class="p-briefh">
                  {{ editOpen ? "новая редакция постановки" : "постановка задачи" }}
                  · создана {{ new Date(prog.task.created_at).toLocaleDateString("ru-RU") }}
                </div>

                <!-- по умолчанию — как было: просто текст -->
                <template v-if="!editOpen">
                  <div class="p-brief">{{ prog.task.business_description || "постановки нет" }}</div>
                  <div v-if="!CLOSED_STATES.has(currentState)" class="p-briefacts">
                    <button class="p-act" @click.stop="startEdit">Править постановку</button>
                    <span class="p-why">правка — новая редакция: путь вернётся к постановщику</span>
                  </div>
                </template>

                <template v-else>
                  <input v-model="editTitle" class="p-brieftitle" spellcheck="false" placeholder="название задачи" />
                  <textarea v-model="editBody" class="p-briefedit" rows="16" spellcheck="false"
                            placeholder="как задача поставлена" />
                  <div class="p-briefacts">
                    <button class="p-act go" :disabled="!canAcceptEdit || busy" @click.stop="acceptEdit">
                      {{ busy ? 'принимаю…' : 'Принять новую редакцию' }}
                    </button>
                    <button class="p-act" @click.stop="cancelEdit">Отмена</button>
                    <span class="p-why">
                      принятие вернёт путь к постановщику; результаты ниже станут несвежими, история останется
                    </span>
                  </div>
                </template>
              </div>



              <!-- реализация: только факты платформы -->
              <div v-if="openTab === 'impl'" class="p-panel">
                <div v-if="!changes" class="p-empty" style="padding: 4px 0">изменений платформа не записала</div>
                <template v-else>
                  <div class="p-kv">
                    <span class="k">ветка</span><span class="v mono">{{ changes.branch ?? "—" }}</span>
                    <span class="k">база → tip</span>
                    <span class="v mono">{{ (changes.bound_head ?? "").slice(0, 8) }} → {{ (changes.tip ?? "").slice(0, 8) }}</span>
                    <span class="k">файлов</span><span class="v">{{ changes.files.length }}</span>
                  </div>
                  <div class="p-sec">что говорит исполнитель о своей работе</div>
                  <div v-if="implBody(s)?.human_paragraph" class="p-lead">{{ implBody(s)!.human_paragraph }}</div>
                  <div v-else class="p-empty" style="padding: 4px 0">у этого прогона отчёта с объяснением нет</div>
                  <details v-if="implBody(s)?.notes" class="p-implrep">
                    <summary>служебные заметки исполнителя</summary>
                    <div class="p-lead mono" style="margin-top:6px;font-size:11px">{{ implBody(s)!.notes }}</div>
                  </details>
                </template>
              </div>

              <!-- worktree: настоящий diff против базы -->
              <div v-if="openTab === 'wt'" class="p-panel">
                <!-- ЖИВОЕ дерево: то, что роль правит прямо сейчас -->
                <div class="p-live">
                  <div class="lv-h">
                    <span class="lv-dot" :class="{ run: live?.running }" />
                    <b>рабочее дерево</b>
                    <span class="mono">{{ live?.branch ?? "—" }}</span>
                    <span class="mono lv-dim">{{ (live?.head ?? "").slice(0, 8) }}</span>
                    <span class="lv-cnt">правок {{ live?.dirty ?? 0 }}</span>
                    <span v-if="live?.running" class="lv-run">живой · обновляется</span>
                    <span v-if="live?.running" class="lv-dim" title="драйвер отдаёт расход токенов файлом по завершении прогона">расход — после прогона</span>
                  </div>
                  <div v-if="liveErr" class="p-empty" style="color:#ffa972;padding:2px 0">дерево не прочиталось: {{ liveErr }}</div>
                  <div v-else-if="live?.note" class="p-empty" style="padding:2px 0">{{ live.note }}</div>
                  <div v-for="f in (live?.files ?? [])" :key="f.path" class="p-lvitem">
                    <div class="p-lvfile" @click="openLiveFile(f.path)">
                      <span class="p-wtst" :class="f.status === '??' ? 'A' : f.status">{{ f.status === "??" ? "A" : f.status }}</span>
                      <span class="p-wtpath mono">{{ f.path }}</span>
                      <span class="lv-num"><i class="add">+{{ f.added }}</i><i class="del">−{{ f.removed }}</i></span>
                    </div>
                    <template v-if="liveOpen === f.path">
                      <div v-if="!liveDiff" class="p-empty" style="padding:4px 10px">читаю…</div>
                      <div v-else-if="!liveDiff.diff" class="p-empty" style="padding:4px 10px">{{ liveDiff.note }}</div>
                      <pre v-else class="p-diff">{{ liveDiff.diff }}</pre>
                    </template>
                  </div>
                  <div v-if="live?.truncated" class="p-empty" style="padding:2px 0">
                    показаны первые {{ live!.files.length }} из {{ live!.dirty }}
                  </div>
                </div>

                <div v-if="changes" class="p-sec">закоммичено · {{ changes.files.length }} файлов</div>
                <div v-if="!changes" class="p-empty" style="padding: 4px 0">в коммите изменений ещё нет — у идущего прогона правки выше, в рабочем дереве</div>
                <template v-else>
                  <div class="p-wthead">
                    <span class="mono">{{ changes.branch ?? "—" }}</span>
                    <span class="mono dim">{{ (changes.bound_head ?? "").slice(0, 8) }} → {{ (changes.tip ?? "").slice(0, 8) }}</span>
                    <span>файлов {{ changes.files.length }}</span>
                  </div>
                  <div v-for="f in changes.files" :key="f.path" class="p-wtfile">
                    <div class="p-wtrow" @click="toggleFile(f.path)">
                      <span class="p-wtst" :class="f.status">{{ f.status }}</span>
                      <span class="p-wtpath mono">{{ f.path }}</span>
                      <span class="p-wtopen">{{ openFile === f.path ? "▾" : "▸" }}</span>
                    </div>
                    <template v-if="openFile === f.path">
                      <div v-if="diffByPath[f.path] === 'loading'" class="p-empty" style="padding: 6px 10px">читаю…</div>
                      <div v-else-if="!(diffByPath[f.path] as any)?.text" class="p-empty" style="padding: 6px 10px">
                        diff недоступен
                      </div>
                      <template v-else>
                        <div v-if="foldsOf(f.path)" class="p-dtools">
                          ⋯ провалов скрыто: {{ foldsOf(f.path) }}
                          <button @click="foldAll(f.path, true)">развернуть весь файл</button>
                          <button @click="foldAll(f.path, false)">свернуть</button>
                        </div>
                        <pre class="p-diff"><span v-for="(l, k) in shown(f.path)" :key="k" class="p-dl" :class="l.cls" @click="l.foldKey !== undefined && unfoldRun(f.path, l.foldKey)">{{ l.text || " " }}</span></pre>
                      </template>
                    </template>
                  </div>
                </template>
              </div>

              <!-- прогоны и артефакты шага -->
              <div v-if="!s.gate && openTab === 'runs'" class="p-panel">
                <div class="p-kv">
                  <span class="k">роль</span><span class="v">{{ s.slug ?? "—" }}@v{{ s.version ?? "—" }}</span>
                </div>
                <div class="p-sec">прогоны · {{ s.execs.length }}</div>
                <div v-for="e in s.execs.slice(-6)" :key="e.id" class="p-exec">
                  <span class="mono">#{{ e.attempt }}</span><span>{{ e.status }}</span>
                  <span class="mono">{{ tokensByExec.get(e.id) ? tok(tokensByExec.get(e.id)) + " ток" : "" }}</span>
                </div>
                <div v-if="!s.execs.length" class="p-empty" style="padding: 4px 0">прогонов не было</div>
                <div class="p-sec">артефакты</div>
                <div v-for="a in (nodes.get(s.state)?.artifacts ?? []).slice(-8)" :key="a.id" class="p-exec">
                  <span>{{ a.display }}</span><span class="mono">{{ a.status }}</span>
                </div>
                <div v-if="!(nodes.get(s.state)?.artifacts ?? []).length" class="p-empty" style="padding: 4px 0">ничего не отдавал</div>
              </div>

              <!-- спецификация (шаг анализа) -->
              <div v-if="openTab === 'spec' && s.state === 'ANALYSIS'" class="p-panel">
                <div v-if="!specBody(s)" class="p-empty" style="padding: 4px 0">
                  у этого прогона спеки нет
                </div>
                <template v-else>
                  <div v-if="specBody(s)!.human_paragraph" class="p-lead">
                    {{ specBody(s)!.human_paragraph }}
                  </div>

                  <!-- изменения к предыдущей версии: галочка, инлайн -->
                  <label v-if="prevRunWithArt(s, 'REQUIREMENTS_SPEC')" class="p-cmp">
                    <input type="checkbox" :checked="!!comparePrev[s.state]" @change="toggleCompare(s)" />
                    <span>что изменилось к предыдущей версии</span>
                    <span class="p-cmpv mono">
                      v{{ prevRunWithArt(s, 'REQUIREMENTS_SPEC')!.art.seq }} → v{{ artOf(s)!.seq }}
                    </span>
                  </label>

                  <template v-if="comparePrev[s.state]">
                    <div v-if="!specCompare(s)" class="p-empty" style="padding: 4px 0">
                      предыдущая версия не прочитана
                    </div>
                    <template v-else>
                      <div class="p-sec">
                        изменено · {{ specCompare(s)!.items.filter((i) => i.kind !== 'same').length }}
                        из {{ specCompare(s)!.items.length }}
                      </div>
                      <div
                        v-for="it in specCompare(s)!.items.filter((i) => i.kind !== 'same')"
                        :key="'d' + it.id"
                        class="p-req p-reqd"
                        :class="it.kind"
                      >
                        <div class="rid"><span class="p-mark">{{ it.kind === 'added' ? '+' : it.kind === 'removed' ? '−' : '~' }}</span>{{ it.id }}</div>
                        <template v-if="it.kind === 'added'">
                          <div class="rst">{{ it.newStatement }}</div>
                        </template>
                        <template v-else-if="it.kind === 'removed'">
                          <div class="rst del">{{ it.oldStatement }}</div>
                        </template>
                        <template v-else>
                          <div v-if="it.statement" class="p-difftext">
                            <div v-for="(l, k) in it.statement" :key="k" class="dl" :class="l.t">{{ l.v || " " }}</div>
                          </div>
                          <ul v-if="(it.ac ?? []).length" class="p-ac">
                            <li v-for="(a, k) in it.ac" :key="k" :class="a.t === 'del' ? 'acdel' : 'acadd'">{{ a.v }}</li>
                          </ul>
                        </template>
                      </div>
                      <template v-if="specCompare(s)!.items.filter((i) => i.kind === 'same').length">
                        <div class="p-sec">
                          без изменений · {{ specCompare(s)!.items.filter((i) => i.kind === 'same').length }}
                        </div>
                        <div v-for="it in specCompare(s)!.items.filter((i) => i.kind === 'same')" :key="'s' + it.id" class="p-req p-same">
                          <div class="rid">{{ it.id }}</div>
                          <div class="rst">{{ it.newStatement }}</div>
                        </div>
                      </template>
                    </template>
                  </template>

                  <template v-else>
                    <div class="p-sec">требований · {{ (specBody(s)!.requirements ?? []).length }}</div>
                    <div v-for="r in specBody(s)!.requirements ?? []" :key="r.id" class="p-req">
                      <div class="rid">{{ r.id }}</div>
                      <div class="rst">{{ r.statement }}</div>
                      <ul v-if="(r.acceptance_criteria ?? []).length" class="p-ac">
                        <li v-for="(a, k) in r.acceptance_criteria" :key="k">{{ a }}</li>
                      </ul>
                    </div>
                  </template>
                </template>
              </div>

              <!-- проверка (шаг ревью) -->
              <div v-if="openTab === 'review'" class="p-panel">
                <div v-if="!reviewBody(s)" class="p-empty" style="padding: 4px 0">
                  у этого прогона отчёта проверки нет
                </div>
                <template v-else>
                  <label v-if="prevRunWithArt(s, 'REVIEW_REPORT')" class="p-cmp">
                    <input type="checkbox" :checked="!!comparePrev[s.state]" @change="toggleCompare(s)" />
                    <span>что изменилось к предыдущей проверке</span>
                    <span class="p-cmpv mono">
                      v{{ prevRunWithArt(s, 'REVIEW_REPORT')!.art.seq }} → v{{ artOf(s)!.seq }}
                    </span>
                  </label>

                  <template v-if="comparePrev[s.state] && reviewCompare(s)">
                    <div class="p-kv">
                      <span class="k">вердикт</span>
                      <span class="v">
                        <template v-if="reviewCompare(s)!.verdictChanged">
                          <span class="vold">{{ reviewCompare(s)!.verdictOld || "—" }}</span>
                          →
                          <b>{{ reviewCompare(s)!.verdictNew || "—" }}</b>
                        </template>
                        <template v-else>{{ reviewCompare(s)!.verdictNew || "—" }} <span class="p-dim2">(не менялся)</span></template>
                      </span>
                      <span v-if="reviewBody(s)!.spec_seq" class="k">проверялась спека</span>
                      <span v-if="reviewBody(s)!.spec_seq" class="v">v{{ reviewBody(s)!.spec_seq }}</span>
                    </div>

                    <template v-if="reviewCompare(s)!.paragraph">
                      <div class="p-sec">формулировка проверки изменилась</div>
                      <div class="p-difftext">
                        <div v-for="(l, k) in reviewCompare(s)!.paragraph!" :key="k" class="dl" :class="l.t">{{ l.v || " " }}</div>
                      </div>
                    </template>

                    <div class="p-sec">
                      замечания · было {{ reviewCompare(s)!.findings.filter((f) => f.t !== 'add').length }},
                      стало {{ reviewCompare(s)!.findings.filter((f) => f.t !== 'del').length }}
                      (новых {{ reviewCompare(s)!.findings.filter((f) => f.t === 'add').length }},
                      снято {{ reviewCompare(s)!.findings.filter((f) => f.t === 'del').length }})
                    </div>
                    <div
                      v-for="(f, k) in reviewCompare(s)!.findings"
                      :key="k"
                      class="p-find p-findd"
                      :class="f.t"
                    >
                      <span class="p-mark">{{ f.t === 'add' ? '+' : f.t === 'del' ? '−' : '·' }}</span>
                      <span class="p-sev" :class="f.severity">{{ f.severity }}</span>
                      <div class="p-findt">
                        <CodeRefText :text="f.text" :task-id="id" :rev="rev" />
                      </div>
                      <span v-if="f.req" class="p-dim2 mono">{{ f.req }}</span>
                    </div>
                  </template>

                  <template v-else>
                  <div class="p-kv">
                    <span class="k">вердикт</span><span class="v">{{ reviewBody(s)!.verdict ?? "—" }}</span>
                    <span v-if="reviewBody(s)!.spec_seq" class="k">проверялась спека</span>
                    <span v-if="reviewBody(s)!.spec_seq" class="v">v{{ reviewBody(s)!.spec_seq }}</span>
                  </div>
                  <div v-if="reviewBody(s)!.human_paragraph" class="p-lead" style="margin-top:8px">
                    {{ reviewBody(s)!.human_paragraph }}
                  </div>
                  <template v-if="(reviewBody(s)!.findings ?? []).length">
                    <div class="p-sec">замечания · {{ (reviewBody(s)!.findings ?? []).length }}</div>
                    <div v-for="(f, k) in reviewBody(s)!.findings" :key="k" class="p-find">
                      <span class="p-sev" :class="f.severity">{{ f.severity }}</span>
                      <div class="p-findt">
                        <CodeRefText :text="String(f.description ?? '')" :task-id="id" :rev="rev" />
                      </div>
                    </div>
                  </template>
                  </template>
                </template>
              </div>

              <!-- что подписываешь -->
              <div v-if="openTab === 'sign' && sigError && !signature" class="p-empty" style="color:#ffa972">
                подпись не прочиталась: {{ sigError }}
              </div>
              <SignPanel
                v-else-if="openTab === 'sign'"
                :signature="signature"
                :busy="signBusy"
                @sign="doSign"
              />

              <!-- качество: сперва ходы человека, ниже — чем он их обосновывает -->
              <div v-if="openTab === 'qa'" class="p-panel">
                <div v-if="qaOpen" class="p-qacts">
                  <button
                    class="p-qa accept"
                    :class="{ holding: qaHolding === 'accept', busy: qaBusy === 'accept' }"
                    :disabled="qaBusy !== null"
                    title="удерживай, чтобы принять задачу"
                    @pointerdown="qaStart('accept', $event)"
                    @pointermove="qaMove($event)"
                    @pointerup="qaRelease"
                    @pointerleave="qaRelease"
                    @pointercancel="qaRelease"
                  >
                    <i class="fill" :style="{ width: (qaHolding === 'accept' ? qaProgress * 100 : 0) + '%' }" />
                    <span>{{ qaBusy === 'accept' ? 'принимаю…' : 'Принять задачу' }}</span>
                  </button>

                  <input
                    v-model="returnNote"
                    class="p-qanote"
                    spellcheck="false"
                    placeholder="что чинить — словами (необязательно; выбранные пункты уйдут сами)"
                  />

                  <button
                    class="p-qa back"
                    :class="{ holding: qaHolding === 'return', busy: qaBusy === 'return' }"
                    :disabled="qaBusy !== null"
                    title="удерживай, чтобы вернуть на доработку"
                    @pointerdown="qaStart('return', $event)"
                    @pointermove="qaMove($event)"
                    @pointerup="qaRelease"
                    @pointerleave="qaRelease"
                    @pointercancel="qaRelease"
                  >
                    <i class="fill" :style="{ width: (qaHolding === 'return' ? qaProgress * 100 : 0) + '%' }" />
                    <span>{{ qaBusy === 'return' ? 'возвращаю…' : `На доработку · ${picked.size}` }}</span>
                  </button>
                </div>
                <div v-if="qaErr" class="p-qerr">{{ qaErr }}</div>

                <!-- влитие: факт на принятой задаче, а не отдельный шаг -->
                <div v-if="prog.task.state === 'ACCEPTED'" class="p-merge" :class="{ done: merged }">
                  <template v-if="merged">
                    <span class="m-dot" />
                    <span class="m-h">влито</span>
                    <span class="m-branch">{{ mergeInfo!.branch }}</span>
                    <span class="m-commit">→ {{ mergeInfo!.commit.slice(0, 7) }}</span>
                    <span class="m-by">{{ mergeInfo!.actor }}</span>
                  </template>
                  <template v-else>
                    <span class="m-h">принята, не влита</span>
                    <span class="m-note">слияние — действие с репозиторием, не стадия процесса</span>
                    <button
                      class="p-mergebtn"
                      :class="{ holding: mergeHolding === 'return' }"
                      :disabled="mergeBusy"
                      title="удерживай, чтобы влить ветку задачи в выложенную baseline"
                      @pointerdown="mergeStart('return', $event)"
                      @pointermove="mergeMove($event)"
                      @pointerup="mergeRelease"
                      @pointerleave="mergeRelease"
                      @pointercancel="mergeRelease"
                    >
                      <i class="fill" :style="{ width: (mergeHolding === 'return' ? mergeProgress * 100 : 0) + '%' }" />
                      <span>{{ mergeBusy ? 'вливаю…' : 'Влить в baseline' }}</span>
                    </button>
                  </template>
                </div>
                <div v-if="mergeErr" class="p-qerr">{{ mergeErr }}</div>

                <div class="p-hero" :class="scoreTone">
                  <div class="big">{{ score === null ? "—" : (score * 100).toFixed(1) }}<span>%</span></div>
                  <div class="side">
                    <div class="lab">общая оценка качества</div>
                    <div class="track">
                      <i :style="{ width: (score === null ? 0 : Math.min(100, score * 100)) + '%' }" />
                      <b :style="{ left: (theta === null ? 80 : Math.min(100, theta * 100)) + '%' }" />
                    </div>
                    <div class="sub">
                      порог {{ theta === null ? "—" : (theta * 100).toFixed(1) }}%
                      <template v-if="score !== null && theta !== null">
                        · {{ score >= theta ? "выше на" : "ниже на" }} {{ Math.abs((score - theta) * 100).toFixed(1) }} пп
                      </template>
                      <template v-if="verdict"> · {{ verdict.toLowerCase() }}</template>
                    </div>
                  </div>
                </div>

                <div class="p-cells">
                  <div class="cell"><div class="k">балл</div><div class="v">{{ score === null ? "—" : score.toFixed(3) }}</div>
                    <div class="n">профиль v{{ rules?.profile_version ?? "—" }}</div></div>
                  <div class="cell"><div class="k">сумма весов</div><div class="v">{{ totalWeight.toFixed(3) }}</div>
                    <div class="n">оценено {{ scoredWeight.toFixed(3) }}</div></div>
                  <div class="cell"><div class="k">пунктов</div><div class="v">{{ checks.length }}</div>
                    <div class="n">{{ checks.filter(c => c.score === 1).length }} по 1.0 · {{ losses }} слабее</div></div>
                  <div class="cell"><div class="k">сумма вклада</div><div class="v">{{ contribSum.toFixed(3) }}</div>
                    <div class="n">потеряно {{ lossSum.toFixed(3) }}</div></div>
                </div>

                <div class="p-tools">
                  <button class="p-tab" :class="{ on: mode === 'loss' }" @click="mode = 'loss'">только потери · {{ losses }}</button>
                  <button class="p-tab" :class="{ on: mode === 'all' }" @click="mode = 'all'">все · {{ checks.length }}</button>
                  <span class="p-pickbar">на доработку <b>{{ picked.size }}</b> · снято с контроля <b>{{ checks.length - picked.size }}</b></span>
                </div>

                <div class="p-qhead">
                  <span /><span>пункт</span><span>важность</span><span>балл</span><span>вклад</span><span>потеря</span>
                </div>

                <div v-for="c in rows" :key="c.id" class="p-qitem"
                     :class="[c.score === null ? 'un' : c.score >= 1 ? 'ok' : c.score >= 0.6 ? 'mid' : 'low', { picked: picked.has(c.id), open: openCheck === c.id }]">
                  <div class="p-qrow" @click="openCheck = openCheck === c.id ? null : c.id">
                    <button class="p-pick" :aria-checked="picked.has(c.id)" @click="togglePick(c, $event)">
                      {{ picked.has(c.id) ? "✕" : "" }}
                    </button>
                    <div class="qid">
                      <span class="qcaret">▶</span>{{ c.id }}
                      <span v-if="c.state && c.state !== 'SCORED'" class="qs2">{{ c.state }}</span>
                      <span v-if="!picked.has(c.id)" class="qfree">снято с контроля</span>
                    </div>
                    <div class="p-bar"><em><i :style="{ width: Math.max(100 * (c.weight ?? 0) / maxWeight, 1.5) + '%' }" /></em><span>{{ (c.weight ?? 0).toFixed(3) }}</span></div>
                    <div class="qscore">{{ c.score === null ? "—" : c.score.toFixed(1) }}</div>
                    <div class="qcontrib">{{ contribText(c) }}</div>
                    <div class="qloss">{{ c.score === null ? "—" : loss(c) > 0 ? "−" + loss(c).toFixed(3) : "0" }}</div>
                  </div>
                  <div v-if="openCheck === c.id" class="p-qbody">
                    <div v-if="ruleFor(c.id)" class="p-rule">
                      <div class="rh">
                        <span class="g">{{ ruleFor(c.id)!.group }}</span>
                        <span>проверка: {{ ruleFor(c.id)!.verify ?? "—" }}</span>
                        <span>вес {{ (ruleFor(c.id)!.weight ?? 0).toFixed(3) }}</span>
                        <span v-if="ruleFor(c.id)!.floor != null" class="floor">пол {{ ruleFor(c.id)!.floor }}</span>
                      </div>
                      <div class="rs">{{ txt(ruleFor(c.id)!.statement) }}</div>
                      <div class="anchors">
                        <div v-for="k in ['1.0','0.7','0.4','0.0'].filter(k => ruleFor(c.id)!.anchors[k] != null)" :key="k"
                             class="anchor" :class="{ on: c.score !== null && Number(k) === Number(c.score) }">
                          <span class="v">{{ k }}</span><span class="t">{{ txt(ruleFor(c.id)!.anchors[k]) }}</span>
                        </div>
                      </div>
                    </div>
                    <div class="p-ev">
                      <CodeRefText :text="evidenceOf(c)" :task-id="id" :rev="rev" />
                    </div>
                  </div>
                </div>
              </div>

              <!-- правила приёмки -->
              <div v-if="openTab === 'rules'" class="p-panel">
                <div class="p-rulehead">
                  правила приёмки · профиль <b>v{{ rules?.profile_version ?? "—" }}</b>
                  · порог <b>{{ rules?.theta ?? "—" }}</b>
                  · вес блокеров ≤ <b>{{ rules?.blocked_max ?? "—" }}</b>
                </div>
                <div v-for="g in rules?.groups ?? []" :key="g.group" class="p-rgroup">
                  <div class="gt">{{ g.group }} · вес {{ g.weight.toFixed(3) }}</div>
                  <div v-for="it in g.items" :key="it.id" class="p-ritem">
                    <div class="rid">{{ it.id }}
                      <span class="rmeta">вес {{ (it.weight ?? 0).toFixed(3) }}{{ it.floor != null ? ` · пол ${it.floor}` : "" }}{{ it.verify ? ` · ${it.verify}` : "" }}</span>
                    </div>
                    <div class="rst">{{ it.statement }}</div>
                    <div class="ranch">
                      <div v-for="k in ['1.0','0.7','0.4','0.0'].filter(k => it.anchors[k] != null)" :key="k" class="anchor">
                        <span class="v">{{ k }}</span><span class="t">{{ it.anchors[k] }}</span>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>
  </section>
</template>

<style scoped>
.p-dhead{display:grid;grid-template-columns:1fr auto auto auto;gap:18px;align-items:center;
  border-bottom:1px solid var(--p-line);padding-bottom:12px;margin-bottom:14px}
.p-dtitle h1{margin:0;font-size:18px;font-weight:600;line-height:1.25;max-width:70ch}
.p-dside{display:flex;flex-direction:column;gap:6px;align-items:flex-end;justify-self:end}
.p-score{text-align:right;min-width:118px;border-left:1px solid var(--p-line);padding-left:16px}
.p-score .n{font-size:34px;font-weight:700;line-height:1;letter-spacing:-1px;color:var(--p-now);font-variant-numeric:tabular-nums}
.p-score.ok .n{color:var(--p-pass)}
.p-score.bad .n{color:var(--p-stop)}
.p-score .n span{font-size:18px;color:var(--p-dim);margin-left:2px}
.p-score .l{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2);margin-top:3px}
.p-score .s{font-size:10px;color:var(--p-dim2);margin-top:2px}
.p-tokens{text-align:right;min-width:146px;border-left:1px solid var(--p-line);padding-left:16px}
.p-tokens .n{font-size:22px;font-weight:650;line-height:1;font-variant-numeric:tabular-nums}
.p-tokens .l{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2);margin-top:3px}
.p-tokens .s{font-size:10px;color:var(--p-dim2);margin-top:2px}

.p-steps{display:flex;flex-direction:column;gap:2px}
.p-vstep{display:grid;grid-template-columns:40px minmax(0,1fr);gap:12px}
.p-nodebox{display:flex;flex-direction:column;align-items:center;padding-top:14px;position:relative}
.p-vnode{width:16px;height:16px;border-radius:50%;border:2px solid #2f3a46;background:#171e26;
  display:flex;align-items:center;justify-content:center;font-size:9px;color:#0c0f13;z-index:2}
.p-vstep.passed .p-vnode{background:var(--p-pass);border-color:var(--p-pass);color:#0c0f13}
.p-vstep.current .p-vnode{width:22px;height:22px;margin-top:-3px;background:var(--p-now);border-color:#ffb98c;box-shadow:0 0 0 5px rgba(243,106,22,.14)}
.p-vstep.gate .p-vnode{border-radius:3px;transform:rotate(45deg)}
.p-vstep.gate .p-vnode span{transform:rotate(-45deg)}
.p-vstep .rail{position:absolute;top:32px;bottom:-2px;width:2px;background:#1f2833}
.p-vstep.passed .rail{background:linear-gradient(#2f9d6e,#1f4f3c)}
/* «Где я сейчас»: узел остаётся оранжевым, даже если стадия уже пройдена —
   на втором круге все узлы зелёные, и без этого не понять, где стоишь. */
.p-vstep.now .p-vnode{width:22px;height:22px;margin-top:-3px;background:var(--p-now);
  border-color:#ffb98c;box-shadow:0 0 0 5px rgba(243,106,22,.16);color:#0c0f13}
.p-vstep.now.gate .p-vnode{border-radius:5px}
.p-vstep.now .p-vcard{border-color:rgba(243,106,22,.45)}
.p-nowchip{font-size:9.5px;letter-spacing:.08em;text-transform:uppercase;padding:2px 6px;border-radius:5px;
  color:#ffb98c;background:rgba(243,106,22,.14);border:1px solid rgba(243,106,22,.4)}
.p-vcard{background:var(--p-card);border:1px solid var(--p-line);border-radius:10px;margin-bottom:8px;overflow:hidden;min-width:0}
.p-vhead{display:grid;grid-template-columns:1fr auto;gap:12px;align-items:center;padding:9px 14px;cursor:pointer;user-select:none}
.p-vhead:hover{background:#172028}
.p-vhead .t1{display:flex;align-items:center;gap:9px;flex-wrap:wrap}
.p-vhead .role{font-size:13.5px;font-weight:600}
.p-vhead .ver{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2)}
.p-spark{display:inline-flex;align-items:flex-end;gap:1px;height:14px;cursor:help}
.p-spark i{width:3px;border-radius:1px 1px 0 0;background:#2b3a4a}
.p-spark i.ok{background:var(--p-pass)} .p-spark i.bad{background:var(--p-stop)}
.p-spark i.mid{background:#4a6b86} .p-spark i.un{background:#2b3440}
.p-human{display:inline-flex;align-items:center;color:var(--p-human);opacity:.85;cursor:help}
.p-failchip{font-size:10px;letter-spacing:.08em;text-transform:uppercase;padding:2px 7px;border-radius:5px;
  color:#ffb3bb;background:rgba(240,97,109,.12);border:1px solid rgba(240,97,109,.38)}
.p-mstats{display:flex;align-items:center;gap:9px;font-family:var(--mono);font-size:11px;color:var(--p-dim2)}
.p-mstats b{color:var(--p-dim);font-weight:500}
.p-mstats .lim{color:#5a6877}



/* ── живое рабочее дерево ───────────────────────────────────────────────── */
.p-live{border:1px solid var(--p-line);border-radius:8px;padding:8px 10px;margin-bottom:10px;background:#0f151c}
.p-live .lv-h{display:flex;align-items:baseline;gap:9px;flex-wrap:wrap;font-size:11.5px;color:#dfe7f0;margin-bottom:5px}
.p-live .lv-dot{width:7px;height:7px;border-radius:50%;background:#3a4654;align-self:center}
.p-live .lv-dot.run{background:var(--p-now);box-shadow:0 0 6px rgba(243,106,22,.7)}
.p-live .lv-dim{color:var(--p-dim2);font-size:10.5px}
.p-live .lv-cnt{color:var(--p-dim2);font-size:10.5px}
.p-live .lv-run{color:var(--p-now);font-size:10px;letter-spacing:.06em;text-transform:uppercase}
.p-lvitem .p-lvfile{cursor:pointer}
.p-lvitem .p-lvfile:hover{background:#141c25}
.p-lvfile{display:flex;gap:9px;align-items:baseline;font-size:11.5px;padding:1px 0}
.p-lvfile .lv-num{margin-left:auto;font-family:var(--mono);font-size:10.5px;display:flex;gap:7px}
.p-lvfile .lv-num .add{color:var(--p-pass);font-style:normal}
.p-lvfile .lv-num .del{color:var(--p-stop);font-style:normal}

/* ── отказ прогона: вкладка внутри шага ─────────────────────────────────── */
.p-runmemo.bad{color:#f0a2a9}
.rf-h{display:flex;gap:9px;align-items:baseline;flex-wrap:wrap;font-size:12px;color:#e9eef4;margin-bottom:8px}
.rf-h .rf-dot{width:7px;height:7px;border-radius:50%;background:var(--p-stop);align-self:center}
.rf-h .rf-cls{color:#f0a2a9}
.rf-h .rf-st{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2)}
.rf-h .rf-att{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2)}
.rf-t{font-family:var(--mono);font-size:11px;line-height:1.5;color:#cfd9e4;background:#080b0e;
  border:1px solid var(--p-line);border-radius:7px;padding:9px 11px;max-height:320px;overflow:auto;
  white-space:pre-wrap;word-break:break-word}
.rf-note{font-size:11px;color:var(--p-dim);margin-bottom:6px}
.rf-acts{display:flex;gap:8px;margin-top:9px}

/* ── консоль прогона ─────────────────────────────────────────────────────── */
.p-termhead{display:flex;align-items:center;gap:9px;font-size:11.5px;color:var(--p-dim);margin-bottom:6px;flex-wrap:wrap}
.p-termhead b{color:#e6edf5;font-weight:600;font-size:12px}
.p-termhead .mono{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2)}
.p-termhead .tdot{width:7px;height:7px;border-radius:50%;background:#3a4654}
.p-termhead .tdot.run{background:var(--p-now);box-shadow:0 0 6px rgba(243,106,22,.7)}
.p-termhead .tdot.done{background:var(--p-pass)}
.p-termhead .tnote{font-size:10.5px;color:#8b97a6}
.p-termhead .p-act{margin-left:auto}
.p-term{background:#080b0e;border:1px solid var(--p-line);border-radius:7px;padding:9px 12px;
  max-height:420px;overflow:auto}

.p-runs{display:flex;align-items:center;gap:3px;flex-wrap:wrap;padding:9px 14px 0}
.p-rlabel{font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--p-dim2);margin-right:6px}
/* Засечка — кнопка с НАСТОЯЩЕЙ областью нажатия: сам штрих узкий (7×14), но
   `padding` даёт палец-размер целиком, иначе по прогонам не попасть. */
.p-rdhit{display:inline-flex;align-items:center;padding:5px 3px;margin:0;background:none;
  border:0;cursor:pointer;line-height:0}
.p-rdhit:hover .p-rd{filter:brightness(1.35)}
.p-rd{display:block;width:7px;height:14px;border-radius:2px;background:#2b3440;
  transition:filter .1s,outline-color .1s}
.p-rd.ok{background:var(--p-pass)} .p-rd.bad{background:var(--p-stop)} .p-rd.run{background:var(--p-now)}
.p-rd.sel{outline:2px solid var(--p-human);outline-offset:1px}
.p-rsel{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2);margin-left:8px}
.p-runmemo{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2);padding:6px 14px 0}
/* сравнение версий спеки */
.p-cmp{display:flex;align-items:center;gap:8px;font-size:12px;color:var(--p-dim);
  border:1px solid var(--p-line);border-radius:8px;padding:7px 11px;background:var(--p-card2);margin:8px 0;cursor:pointer}
.p-cmp input{accent-color:#7aa7ff}
.p-cmpv{margin-left:auto;font-size:11px;color:var(--p-dim2)}
.p-reqd{border-color:rgba(243,106,22,.35)}
.p-reqd.added{border-color:rgba(62,207,142,.4)}
.p-reqd.removed{border-color:rgba(240,97,109,.4)}
.p-same{opacity:.55}
.p-mark{display:inline-block;width:12px;color:var(--p-now);font-weight:600}
.p-reqd.added .p-mark{color:var(--p-pass)} .p-reqd.removed .p-mark{color:var(--p-stop)}
.p-difftext{font-family:var(--mono);font-size:11.5px;line-height:1.5;margin-top:4px}
.p-difftext .dl{padding:1px 8px;border-radius:3px;white-space:pre-wrap}
.p-difftext .dl.add{background:rgba(62,207,142,.10);color:#b8e8cd}
.p-difftext .dl.del{background:rgba(240,97,109,.10);color:#f2bcc2;text-decoration:line-through}
.p-difftext .dl.same{color:var(--p-dim)}
.p-ac .acadd{color:#b8e8cd} .p-ac .acdel{color:#f2bcc2;text-decoration:line-through}
.p-dim2{color:var(--p-dim2)} .mono{font-family:var(--mono)}
.vold{color:var(--p-stop);text-decoration:line-through;margin-right:6px}
.p-findd{align-items:baseline}
.p-findd.add{background:rgba(62,207,142,.07);border-left:2px solid rgba(62,207,142,.5);padding-left:8px}
.p-findd.del{background:rgba(240,97,109,.07);border-left:2px solid rgba(240,97,109,.5);padding-left:8px}
.p-findd.del .p-findt{color:#f2bcc2;text-decoration:line-through}
.p-findd.same .p-findt{color:var(--p-dim)}
.p-findt{flex:1}
.p-coderef{background:rgba(122,167,255,.10);border:1px solid rgba(122,167,255,.35);
  color:var(--p-human);border-radius:4px;padding:0 5px;font-family:var(--mono);
  font-size:11px;cursor:pointer;margin:0 2px}
.p-coderef:hover{background:rgba(122,167,255,.22)}
.p-coderef.on{background:rgba(62,207,142,.2);border-color:rgba(62,207,142,.6);color:#8fe0b7}
.p-find{display:flex;gap:8px;font-size:12px;color:#dbe3ec;margin-bottom:6px;line-height:1.45}
.p-sev{font-family:var(--mono);font-size:10px;text-transform:uppercase;padding:1px 6px;border-radius:4px;
  border:1px solid var(--p-line);color:var(--p-dim);height:16px}
.p-sev.blocker{color:var(--p-stop);border-color:rgba(240,97,109,.4)}
.p-sev.major{color:var(--p-now);border-color:rgba(243,106,22,.4)}
.p-vhead .t2{display:flex;align-items:center;gap:10px}
.p-chip{font-size:10.5px;color:var(--p-dim);border:1px solid var(--p-line);border-radius:5px;padding:1px 7px;white-space:nowrap}
.p-chip.good{color:var(--p-pass);border-color:rgba(62,207,142,.3)}
.p-chip.hot{color:var(--p-now);border-color:rgba(243,106,22,.3)}
.p-scorechip{display:inline-flex;align-items:baseline;gap:5px;font-size:14px;font-weight:700;padding:2px 9px;
  border-radius:6px;border:1px solid;color:var(--p-now);border-color:rgba(243,106,22,.35);background:rgba(243,106,22,.08)}
.p-scorechip.ok{color:var(--p-pass);border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.08)}
.p-scorechip.bad{color:var(--p-stop);border-color:rgba(240,97,109,.35);background:rgba(240,97,109,.08)}
.p-scorechip i{font-size:9.5px;font-style:normal;font-weight:500;letter-spacing:.06em;text-transform:uppercase;opacity:.75}
.p-act{font:inherit;font-size:11px;padding:3px 10px;border-radius:6px;cursor:pointer;border:1px solid var(--p-line);background:#172028;color:#cfd8e2}
.p-act.run{border-color:rgba(62,207,142,.45);color:#8fe3ba;background:rgba(62,207,142,.08)}
.p-act.stop{border-color:rgba(240,97,109,.45);color:#ffb3bb;background:rgba(240,97,109,.08)}
.p-why{font-size:10px;color:var(--p-dim2)}
.caret{color:var(--p-dim2);font-size:9px}
.p-vstep.open .caret{color:var(--p-ink)}
.p-tabs{display:flex;gap:6px;padding:9px 14px;border-bottom:1px solid var(--p-line);background:var(--p-card2)}
.p-tab{font:inherit;font-size:11.5px;padding:3px 10px;border-radius:6px;border:1px solid var(--p-line);background:transparent;color:var(--p-dim);cursor:pointer}
.p-tab.on{color:var(--p-ink);background:#1b242e;border-color:#33414f}
.p-panel{padding:14px 16px 18px}
.p-implrep{margin-top:10px;border:1px solid var(--p-line);border-radius:8px;background:var(--p-card2);padding:8px 11px}
.p-implrep summary{cursor:pointer;font-size:11.5px;color:var(--p-dim);list-style:none}
.p-implrep summary::-webkit-details-marker{display:none}
.p-implrep summary:before{content:'▸ ';color:var(--p-dim2)}
.p-wthead{display:flex;gap:14px;align-items:baseline;flex-wrap:wrap;font-size:11.5px;color:var(--p-dim);
  background:var(--p-card2);border:1px solid var(--p-line);border-radius:8px;padding:8px 11px;margin-bottom:10px}
.p-wthead .dim{color:var(--p-dim2)}
.p-wtfile{border:1px solid var(--p-line);border-radius:8px;margin-bottom:6px;background:var(--p-card2);overflow:hidden}
.p-wtrow{display:flex;gap:10px;align-items:center;padding:7px 11px;cursor:pointer;font-size:12px}
.p-wtrow:hover{background:rgba(122,167,255,.05)}
.p-wtst{width:16px;text-align:center;font-family:var(--mono);font-size:11px}
.p-wtst.A{color:var(--p-pass)} .p-wtst.M{color:var(--p-now)} .p-wtst.D{color:var(--p-stop)}
.p-wtpath{flex:1;font-size:11.5px;color:#cfd8e2;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.p-wtopen{color:var(--p-dim2);font-size:11px}
.p-diff{margin:0;padding:8px 0;background:#0b0e12;border-top:1px solid var(--p-line);
  font-family:var(--mono);font-size:11.5px;line-height:1.5;overflow:auto;max-height:520px}
.p-dl{display:block;padding:0 12px;white-space:pre}
.p-dl.add{background:rgba(62,207,142,.10);color:#b8e8cd}
.p-dl.del{background:rgba(240,97,109,.10);color:#f2bcc2}
.p-dl.hunk{color:var(--p-human);background:rgba(122,167,255,.07)}
.p-dl.meta{color:var(--p-dim2)}
.p-dl.fold{color:var(--p-human);background:rgba(122,167,255,.06);cursor:pointer;
  border-top:1px dashed rgba(122,167,255,.25);border-bottom:1px dashed rgba(122,167,255,.25);
  padding:2px 12px;font-size:11px}
.p-dl.fold:hover{background:rgba(122,167,255,.13)}
.p-dtools{display:flex;gap:8px;align-items:center;padding:6px 11px;background:#0e141a;
  border-top:1px solid var(--p-line);font-size:11px;color:var(--p-dim2)}
.p-dtools button{background:transparent;border:1px solid var(--p-line);color:var(--p-human);
  border-radius:6px;padding:2px 8px;font-size:11px;cursor:pointer}
.p-dtools button:hover{background:rgba(122,167,255,.1)}
.p-briefh{font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2);margin-bottom:8px}
.p-brief{font-size:13px;line-height:1.6;color:#d6dfe8;white-space:pre-wrap;background:var(--p-card2);
  border:1px solid var(--p-line);border-radius:8px;padding:12px 14px;max-width:120ch}
.p-lead{font-size:13px;line-height:1.55;color:#d3dce6;max-width:120ch}
.p-req{border:1px solid var(--p-line);border-radius:8px;padding:10px 12px;margin-bottom:8px;background:var(--p-card2)}
.p-req .rid{font-family:var(--mono);font-size:11px;color:var(--p-human)}
.p-req .rst{margin-top:4px;font-size:12.5px;color:#dbe3ec;line-height:1.45}
.p-ac{margin:8px 0 0;padding-left:16px;color:var(--p-dim);font-size:12px}
.p-ac li{margin-bottom:3px}
.p-oq{border-left:2px solid var(--p-now);padding:4px 0 4px 10px;background:rgba(243,106,22,.05);
  font-size:12px;color:#e8dcc4;margin-bottom:6px}
.p-kv{display:grid;grid-template-columns:130px 1fr;gap:4px 12px;font-size:12px}
.p-kv .k{color:var(--p-dim2)}
.p-kv .v{color:#d3dce6}
.p-sec{margin-top:12px;font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2)}
.p-exec{display:flex;gap:12px;font-size:11.5px;color:#c3ccd6;padding:2px 0;font-variant-numeric:tabular-nums}


/* ── ходы приёмки: принять или вернуть ──────────────────────────────────── */

/* ── влитие: факт, а не шаг ─────────────────────────────────────────────── */
.p-merge{display:flex;align-items:center;gap:9px;flex-wrap:wrap;margin-bottom:10px;padding:8px 11px;
  border:1px solid var(--p-line);border-radius:8px;background:#10171f;font-size:12px}
.p-merge.done{border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.06)}
.p-merge .m-dot{width:7px;height:7px;border-radius:50%;background:var(--p-pass)}
.p-merge .m-h{font-size:10.5px;letter-spacing:.09em;text-transform:uppercase;color:#93a1b1}
.p-merge.done .m-h{color:var(--p-pass)}
.p-merge .m-branch,.p-merge .m-commit{font-family:var(--mono);font-size:11px;color:#dfe7f0}
.p-merge .m-by{font-size:11px;color:var(--p-dim2)}
.p-merge .m-note{font-size:11px;color:var(--p-dim2)}
.p-mergebtn{position:relative;overflow:hidden;margin-left:auto;font:inherit;font-size:11.5px;padding:5px 12px;
  border-radius:6px;cursor:pointer;border:1px solid rgba(62,207,142,.45);color:#8fe3ba;background:rgba(62,207,142,.10)}
.p-mergebtn .fill{position:absolute;left:0;top:0;bottom:0;width:0;background:rgba(62,207,142,.22)}
.p-mergebtn span{position:relative}
.p-mergebtn.holding{border-color:var(--p-human)}
.p-mergebtn:disabled{opacity:.55;cursor:default}

.p-qacts{display:flex;align-items:center;gap:8px;margin-bottom:10px;flex-wrap:wrap}
.p-qa{position:relative;overflow:hidden;font:inherit;font-size:12px;padding:7px 14px;border-radius:7px;
  cursor:pointer;border:1px solid var(--p-line);background:#172028;color:#cfd8e2;white-space:nowrap}
.p-qa .fill{position:absolute;left:0;top:0;bottom:0;width:0;background:rgba(255,255,255,.10);transition:width .05s linear}
.p-qa span{position:relative}
.p-qa.accept{border-color:rgba(62,207,142,.45);color:#8fe3ba;background:rgba(62,207,142,.10)}
.p-qa.accept .fill{background:rgba(62,207,142,.22)}
.p-qa.back{border-color:rgba(243,106,22,.45);color:#ffb98c;background:rgba(243,106,22,.10)}
.p-qa.back .fill{background:rgba(243,106,22,.22)}
.p-qa.holding{border-color:var(--p-human)}
.p-qa:disabled{opacity:.55;cursor:default}
.p-qanote{flex:1;min-width:220px;font:inherit;font-size:12px;padding:7px 10px;border-radius:7px;
  border:1px solid var(--p-line);background:#0f151c;color:#e9eef4}
.p-qanote:focus{outline:none;border-color:#3a4756}
.p-qerr{font-size:11.5px;color:#ffb3bb;margin-bottom:8px}

.p-hero{display:flex;align-items:center;gap:22px;border:1px solid var(--p-line);border-radius:10px;
  padding:12px 16px;background:linear-gradient(90deg, rgba(243,106,22,.06), transparent 70%)}
.p-hero .big{font-size:50px;font-weight:700;letter-spacing:-2px;line-height:1;color:var(--p-now);font-variant-numeric:tabular-nums}
.p-hero.ok .big{color:var(--p-pass)}
.p-hero.bad .big{color:var(--p-stop)}
.p-hero .big span{font-size:22px;color:var(--p-dim);margin-left:3px}
.p-hero .lab{font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--p-dim2)}
.p-hero .track{position:relative;height:10px;border-radius:5px;background:#1b232c;margin:8px 0 6px;max-width:520px}
.p-hero .track i{position:absolute;left:0;top:0;bottom:0;border-radius:5px;background:var(--p-now)}
.p-hero.ok .track i{background:var(--p-pass)}
.p-hero.bad .track i{background:var(--p-stop)}
.p-hero .track b{position:absolute;top:-3px;width:2px;height:16px;background:var(--p-ink);opacity:.6}
.p-hero .sub{font-size:11px;color:var(--p-dim)}
.p-cells{display:grid;grid-template-columns:repeat(4,1fr);gap:8px;margin:10px 0}
.p-cells .cell{border:1px solid var(--p-line);border-radius:8px;padding:9px 11px;background:var(--p-card2)}
.p-cells .k{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2)}
.p-cells .v{font-size:19px;font-weight:650;margin-top:3px;font-variant-numeric:tabular-nums}
.p-cells .n{font-size:10.5px;color:var(--p-dim2);margin-top:2px}
.p-tools{display:flex;align-items:center;gap:8px;margin:4px 0 8px;flex-wrap:wrap}
.p-pickbar{font-size:11px;color:var(--p-dim2);margin-left:auto}
.p-pickbar b{color:var(--p-ink);font-variant-numeric:tabular-nums}
.p-qhead,.p-qrow{display:grid;grid-template-columns:24px 1fr 150px 52px 64px 64px;gap:8px;align-items:center}
.p-qhead{font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--p-dim2);padding:0 2px}
.p-qitem{border-bottom:1px solid #161d25;border-left:2px solid transparent;padding:2px 0}
.p-qitem.picked{border-left-color:var(--p-stop);background:rgba(240,97,109,.04)}
.p-qitem:not(.picked){opacity:.72}
.p-qrow{padding:6px 2px;font-size:12px;cursor:pointer;font-variant-numeric:tabular-nums}
.p-pick{width:18px;height:18px;padding:0;border-radius:4px;border:1px solid #33414f;background:#10151b;color:var(--p-stop);font-size:11px;cursor:pointer;line-height:1}
.p-qitem.picked .p-pick{border-color:var(--p-stop);background:rgba(240,97,109,.16)}
.qid{font-family:var(--mono);font-size:11.5px;color:#cfd8e2;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.qcaret{display:inline-block;width:12px;color:var(--p-dim2);font-size:9px}
.qs2{font-size:9.5px;color:var(--p-now);border:1px solid rgba(243,106,22,.3);border-radius:4px;padding:0 4px}
.qfree{font-size:9.5px;color:var(--p-dim2);border:1px dashed var(--p-line);border-radius:4px;padding:0 5px;margin-left:6px}
.p-bar{display:flex;align-items:center;gap:7px}
.p-bar em{position:relative;flex:1;height:7px;border-radius:4px;background:#222c37;font-style:normal}
.p-bar em i{position:absolute;left:0;top:0;bottom:0;border-radius:4px;background:#5d8cc4}
.p-qitem.ok .p-bar em i{background:#2f7d5c}
.p-qitem.mid .p-bar em i{background:#a8802f}
.p-qitem.low .p-bar em i{background:#b0475a}
.p-bar span{font-size:10.5px;color:var(--p-dim2);min-width:34px;text-align:right}
.qscore{color:#cfd8e2}
.p-qitem.ok .qscore{color:var(--p-pass)}
.p-qitem.mid .qscore{color:var(--p-now)}
.p-qitem.low .qscore{color:var(--p-stop)}
.qcontrib{color:#cfd8e2}
.qloss{color:var(--p-stop)}
.p-qitem.ok .qloss{color:var(--p-dim2)}
.p-qbody{padding:2px 0 10px 14px;border-left:2px solid #1c2632;margin:2px 0 4px 2px}
.p-rule{background:#0f141a;border:1px solid var(--p-line);border-radius:8px;padding:9px 11px;margin-bottom:8px}
.p-rule .rh{display:flex;gap:10px;flex-wrap:wrap;font-size:10.5px;color:var(--p-dim2)}
.p-rule .rh .g{color:var(--p-human)}
.p-rule .floor{color:var(--p-stop)}
.p-rule .rs{font-size:12px;color:#d7e0ea;line-height:1.5;margin-top:5px;max-width:130ch}
.anchors{margin-top:7px;display:flex;flex-direction:column;gap:2px}
.anchor{display:flex;gap:8px;font-size:11px;color:var(--p-dim2)}
.anchor.on{color:var(--p-ink);border-left:2px solid var(--p-now);padding-left:6px;margin-left:-8px;background:rgba(243,106,22,.07)}
.anchor .v{font-family:var(--mono);min-width:26px}
.p-ev{font-size:11.5px;color:#9aa7b5;line-height:1.5;white-space:pre-wrap;max-width:140ch}
.p-rulehead{background:var(--p-card2);border:1px solid var(--p-line);border-radius:8px;padding:9px 11px;font-size:11.5px;color:#c3cedb;margin-bottom:10px}
.p-rgroup{margin-bottom:12px}
.p-rgroup .gt{font-size:11px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-human);margin-bottom:6px}
.p-ritem{border:1px solid var(--p-line);border-radius:8px;padding:8px 11px;margin-bottom:6px;background:var(--p-card2)}
.p-ritem .rid{font-family:var(--mono);font-size:11.5px;color:#cfd8e2}
.p-ritem .rmeta{font-size:10.5px;color:var(--p-dim2);margin-left:8px}
.p-ritem .rst{font-size:12px;color:#cdd7e2;line-height:1.5;margin-top:5px;max-width:140ch}

@media (max-width: 1199px) {
  .p-dhead{grid-template-columns:1fr}
  .p-dside{flex-direction:row;align-items:center}
  .p-score,.p-tokens{border-left:0;padding-left:0;text-align:left}
  .p-qhead,.p-qrow{grid-template-columns:24px 1fr 90px 44px 56px 56px}
  .p-chip,.p-qhead,.p-bar span,.qs2,.qfree,.p-why,.caret,.p-why,.p-act,.p-tab,.p-scorechip i{font-size:12px}
  .p-sec,.p-cells .k,.p-cells .n,.p-pickbar,.p-rule .rh,.anchor,.p-ritem .rmeta{font-size:12px}
  .p-vhead .ver,.qid,.p-exec,.p-ev,.p-rule .rs,.p-ritem .rst{font-size:12px}
}
/* ── навигация карточки: возврат к списку и закрытие дела ──────────────── */
.p-dnav{display:flex;align-items:center;justify-content:space-between;gap:10px;margin-bottom:10px}
.p-dright{display:flex;align-items:center;gap:8px}
.p-back{border:1px solid var(--p-line);border-radius:7px;background:#131a22;color:var(--p-dim);
  font-family:var(--sans);font-size:12px;padding:5px 11px;cursor:pointer}
.p-back:hover{color:var(--p-ink);border-color:#3a4753}
.p-cancel{position:relative;overflow:hidden;display:inline-flex;align-items:center;gap:6px;
  border:1px solid rgba(240,97,109,.55);border-radius:7px;background:rgba(240,97,109,.08);
  color:#f0a3ab;font-family:var(--sans);font-size:12px;padding:5px 13px;cursor:pointer;
  user-select:none;touch-action:none}
.p-cancel .fill{position:absolute;left:0;top:0;bottom:0;width:0;background:rgba(240,97,109,.45);
  transition:width .04s linear;pointer-events:none}
.p-cancel span{position:relative}
.p-cancel:hover:not(:disabled){border-color:rgba(240,97,109,.9);color:#ffc2c8}
.p-cancel:disabled{opacity:.4;cursor:not-allowed}.p-resume{border:1px solid rgba(62,207,142,.55);border-radius:7px;background:rgba(62,207,142,.12);
  color:#8ee7bc;font-family:var(--sans);font-size:12px;padding:5px 13px;cursor:pointer}
.p-resume:hover:not(:disabled){background:rgba(62,207,142,.22)}
.p-resume:disabled{opacity:.45;cursor:not-allowed}
/* ── перепройти шаг / новая редакция постановки ─────────────────────────── */
.p-act.replay{border-color:rgba(243,106,22,.55);color:#ffc09a;background:rgba(243,106,22,.1)}
.p-act.replay:hover:not(:disabled){background:rgba(243,106,22,.2)}
.p-briefnote{margin-left:8px;color:var(--p-dim2);text-transform:none;letter-spacing:0}
.p-brieftitle{display:block;width:100%;box-sizing:border-box;border:1px solid var(--p-line);
  border-radius:7px;background:#0b0e12;color:var(--p-ink);font-family:var(--sans);
  font-size:13.5px;font-weight:600;padding:7px 10px;margin-bottom:7px}
.p-briefedit{display:block;width:100%;box-sizing:border-box;resize:vertical;min-height:340px;
  border:1px solid var(--p-line);border-radius:7px;background:#0b0e12;color:#dbe3ec;
  font-family:var(--sans);font-size:13px;line-height:1.55;padding:10px 12px}
.p-brieftitle:focus,.p-briefedit:focus{outline:none;border-color:var(--p-human)}
.p-briefacts{display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-top:8px}
.p-act.go{border-color:rgba(62,207,142,.55);background:rgba(62,207,142,.13);color:#8ee7bc;font-weight:600}
.p-act.go:hover:not(:disabled){background:rgba(62,207,142,.22)}
.p-start{border:1px solid #3ecf8e;border-radius:7px;background:rgba(62,207,142,.2);
  color:#b6f2d5;font-family:var(--sans);font-size:12.5px;font-weight:600;
  padding:6px 15px;cursor:pointer}
.p-start:hover:not(:disabled){background:rgba(62,207,142,.32)}
.p-start:disabled{opacity:.5;cursor:not-allowed}
</style>
