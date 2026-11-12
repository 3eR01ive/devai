<script setup lang="ts">
// Новая карточка задачи (по макету): вертикальный путь, один открытый шаг,
// «вход/выход» у ролей, «качество» и «правила приёмки» у гейтов; расход
// токенов по шагам, вклад каждого пункта приёмки и вставки кода.
import { computed, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { api, toUiError } from "../api";
import type {
  ProgressView, QualityReportView, RulebookView, SignatureView, TaskUsageView,
  UiError, WorkspaceChangesView, WorkspaceDiffView,
} from "../types";
import CodeSlice from "./CodeSlice.vue";
import "./proto.css";

const route = useRoute();
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
// Проверки нормализуем один раз: у непроверенного пункта балла может не быть
// вовсе (undefined), а не только null — иначе сумма потерь даёт NaN.
const checks = computed<any[]>(() =>
  (body.value.evaluation?.checks ?? []).map((c: any) => ({
    ...c,
    score: typeof c.score === "number" ? c.score : null,
    weight: typeof c.weight === "number" ? c.weight : 0,
  })),
);
const headline = computed<Record<string, any>>(() => body.value.human_view?.headline ?? {});
const score = computed<number | null>(
  () => (typeof headline.value.score === "number" ? headline.value.score : null),
);
const theta = computed<number | null>(
  () => (typeof headline.value.theta === "number" ? headline.value.theta : null),
);
const verdict = computed<string>(() => String(headline.value.verdict ?? ""));
const rev = computed<string>(() => body.value.workspace_revision?.git_head ?? "");

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

// ── шаги ────────────────────────────────────────────────────────────────
const nodes = computed(() => new Map((prog.value?.graph.nodes ?? []).map((n) => [n.state, n])));

const steps = computed(() => {
  const g = prog.value?.graph;
  if (!g) return [];
  const allowed = prog.value?.allowed_operations ?? [];
  return g.stages.map((s) => {
    const n = nodes.value.get(s.state);
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
  // Постановка показывает сам текст постановки — вкладки ей не нужны.
  if (s.state === "STATEMENT") return [] as [string, string][];
  // Подпись показывает документ, который подписывают.
  if (s.state === "APPROVAL") return [["sign", "что подписываешь"]] as [string, string][];
  if (s.gate) {
    const t: [string, string][] = [];
    if (s.state === "QA" && report.value) {
      t.push(["qa", "качество"]);
      t.push(["rules", "правила приёмки"]);
    }
    return t;
  }
  // «вход/выход» убраны: служебные контракты роли человеку ничего не объясняют.
  // У шага — что произведено (прогоны, артефакты) и что этот шаг решает.
  if (s.state === "ANALYSIS" && signature.value?.spec) {
    return [["spec", "спецификация"], ["runs", "прогоны"]] as [string, string][];
  }
  if (s.state === "REVIEW" && signature.value?.review) {
    return [["review", "проверка"], ["runs", "прогоны"]] as [string, string][];
  }
  if (s.state === "IMPLEMENTATION") {
    return [["impl", "реализация"], ["wt", "worktree"], ["runs", "прогоны"]] as [string, string][];
  }
  return [["runs", "прогоны"]] as [string, string][];
};

function openStep(s: any) {
  if (openState.value === s.state) { openState.value = null; return; }
  openState.value = s.state;
  if (s.state === "IMPLEMENTATION") {
    void loadChanges();
    void loadImplReport();
  }
  const t = tabsFor(s);
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

watch(checks, (list) => {
  if (list.length && picked.value.size === 0) picked.value = new Set(list.map((c) => c.id));
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

function pieces(text: string): Piece[] {
  const out: Piece[] = [];
  let last = 0;
  for (const m of text.matchAll(REF)) {
    const [raw, path, a, b] = m;
    const start = m.index ?? 0;
    if (start < last) continue;
    if (start > last) out.push({ t: "text", v: text.slice(last, start) });
    out.push({
      t: "code",
      path: String(path ?? ""),
      from: a ? Number(a) : 1,
      to: b ? Number(b) : a ? Number(a) + 11 : 12,
    });
    last = start + raw.length;
  }
  if (last < text.length) out.push({ t: "text", v: text.slice(last) });
  return out;
}

function evidenceOf(c: any): string {
  const e = (c.violations?.[0] ?? c.evidence?.[0] ?? "") as string;
  return String(e).slice(0, 1200);
}

// ── данные ──────────────────────────────────────────────────────────────
async function load() {
  error.value = null;
  try {
    const [p, r, u, rb, sig] = await Promise.all([
      api.taskProgress(id.value),
      api.taskReport(id.value).catch(() => null),
      api.taskUsage(id.value).catch(() => null),
      api.evaluationRules().catch(() => null),
      api.taskSignature(id.value).catch(() => null),
    ]);
    prog.value = p;
    report.value = r;
    usage.value = u;
    rules.value = rb;
    signature.value = sig;
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
async function doStop(execId: string) {
  busy.value = true;
  try { await api.execCancel(execId); await load(); }
  catch (e) { error.value = toUiError(e); }
  finally { busy.value = false; }
}

onMounted(load);
</script>

<template>
  <section class="p-root">
    <div v-if="error" class="p-err">{{ error.code }} — {{ error.message }}</div>
    <div v-else-if="!prog" class="p-empty">читаю задачу…</div>

    <template v-else>
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

      <div class="p-steps">
        <div
          v-for="(s, i) in steps"
          :key="s.state"
          class="p-vstep"
          :class="[s.status, { gate: s.gate, open: openState === s.state }]"
        >
          <div class="p-nodebox">
            <div class="p-vnode">{{ s.status === "passed" ? "✓" : "" }}</div>
            <div v-if="i < steps.length - 1" class="rail" />
          </div>
          <div class="p-vcard">
            <div class="p-vhead" @click="openStep(s)">
              <div class="t1">
                <span class="role">{{ s.title }}</span>
                <span v-if="s.slug" class="ver">{{ s.slug }}@v{{ s.version }}</span>
                <span v-if="s.score !== null" class="p-scorechip" :class="scoreTone">
                  {{ (s.score * 100).toFixed(1) }}%<i>качество</i>
                </span>
                <span class="p-chip" :class="s.status === 'passed' ? 'good' : s.status === 'current' ? 'hot' : ''">{{ s.word }}</span>
                <span v-if="s.execs.length" class="p-chip">прогонов {{ s.execs.length }}</span>
                <span v-if="s.tokens" class="p-chip">{{ tok(s.tokens) }} ток</span>
                <span v-if="s.gate" class="p-chip hot">решает человек</span>
              </div>
              <div class="t2">
                <button v-if="s.running" class="p-act stop" :disabled="busy" @click.stop="doStop(s.runningId!)">■ остановить</button>
                <button v-else-if="s.canRun" class="p-act run" :disabled="busy" :title="s.runOp" @click.stop="doRun()">▶ запустить</button>
                <span v-else-if="s.status === 'current'" class="p-why">{{ s.why }}</span>
                <span class="caret">▶</span>
              </div>
            </div>

            <div v-if="openState === s.state" class="p-vbody">
              <!-- постановка: исходный текст задачи, без служебных вкладок -->
              <div v-if="s.state === 'STATEMENT'" class="p-panel">
                <div class="p-briefh">
                  исходная постановка · создана {{ new Date(prog.task.created_at).toLocaleDateString("ru-RU") }}
                </div>
                <div class="p-brief">{{ prog.task.business_description || "постановки нет" }}</div>
              </div>

              <div v-if="tabsFor(s).length" class="p-tabs">
                <button
                  v-for="t in tabsFor(s)"
                  :key="t[0]"
                  class="p-tab"
                  :class="{ on: openTab === t[0] }"
                  @click="openTab = t[0]"
                >{{ t[1] }}</button>
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
                  <div v-if="implWords?.paragraph" class="p-lead">{{ implWords.paragraph }}</div>
                  <div v-else class="p-empty" style="padding: 4px 0">отчёта с объяснением нет</div>
                  <details v-if="implWords?.notes" class="p-implrep">
                    <summary>служебные заметки исполнителя</summary>
                    <div class="p-lead mono" style="margin-top:6px;font-size:11px">{{ implWords.notes }}</div>
                  </details>
                </template>
              </div>

              <!-- worktree: настоящий diff против базы -->
              <div v-if="openTab === 'wt'" class="p-panel">
                <div v-if="!changes" class="p-empty" style="padding: 4px 0">изменений платформа не записала</div>
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
                <div v-if="!signature?.spec" class="p-empty" style="padding: 4px 0">спеки нет</div>
                <template v-else>
                  <div v-if="signature.spec.human_paragraph" class="p-lead">
                    {{ signature.spec.human_paragraph }}
                  </div>
                  <div class="p-sec">требований · {{ signature.spec.requirements.length }}</div>
                  <div v-for="r in signature.spec.requirements" :key="r.id" class="p-req">
                    <div class="rid">{{ r.id }}</div>
                    <div class="rst">{{ r.statement }}</div>
                    <ul v-if="r.acceptance_criteria.length" class="p-ac">
                      <li v-for="(a, k) in r.acceptance_criteria" :key="k">{{ a }}</li>
                    </ul>
                  </div>
                </template>
              </div>

              <!-- проверка (шаг ревью) -->
              <div v-if="openTab === 'review'" class="p-panel">
                <div v-if="!signature?.review" class="p-empty" style="padding: 4px 0">отчёта ревью нет</div>
                <template v-else>
                  <div class="p-kv">
                    <span class="k">вердикт</span><span class="v">{{ signature.review.verdict ?? "—" }}</span>
                  </div>
                  <div v-if="signature.review.human_paragraph" class="p-lead" style="margin-top:8px">
                    {{ signature.review.human_paragraph }}
                  </div>
                </template>
              </div>

              <!-- что подписываешь -->
              <div v-if="openTab === 'sign'" class="p-panel">
                <div v-if="!signature?.spec" class="p-empty" style="padding: 4px 0">
                  спеки нет — подписывать нечего
                </div>
                <template v-else>
                  <div v-if="signature.spec.human_paragraph" class="p-lead">
                    {{ signature.spec.human_paragraph }}
                  </div>
                  <div class="p-sec">требований · {{ signature.spec.requirements.length }}</div>
                  <div v-for="r in signature.spec.requirements" :key="r.id" class="p-req">
                    <div class="rid">{{ r.id }}</div>
                    <div class="rst">{{ r.statement }}</div>
                    <ul v-if="r.acceptance_criteria.length" class="p-ac">
                      <li v-for="(a, k) in r.acceptance_criteria" :key="k">{{ a }}</li>
                    </ul>
                  </div>
                  <div v-if="signature.spec.open_questions.length" class="p-sec">
                    вопросы в документе · {{ signature.spec.open_questions.length }}
                  </div>
                  <div v-for="(q, k) in signature.spec.open_questions" :key="'oq' + k" class="p-oq">{{ q }}</div>
                </template>
              </div>

              <!-- качество -->
              <div v-if="openTab === 'qa'" class="p-panel">
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
                      <div class="rs">{{ ruleFor(c.id)!.statement }}</div>
                      <div class="anchors">
                        <div v-for="k in ['1.0','0.7','0.4','0.0'].filter(k => ruleFor(c.id)!.anchors[k] != null)" :key="k"
                             class="anchor" :class="{ on: c.score !== null && Number(k) === Number(c.score) }">
                          <span class="v">{{ k }}</span><span class="t">{{ ruleFor(c.id)!.anchors[k] }}</span>
                        </div>
                      </div>
                    </div>
                    <div class="p-ev">
                      <template v-for="(p, i) in pieces(evidenceOf(c))" :key="i">
                        <span v-if="p.t === 'text'">{{ p.v }}</span>
                        <CodeSlice v-else :task-id="id" :rev="rev" :path="p.path" :from="p.from" :to="p.to" />
                      </template>
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
.p-vstep.current .p-vnode{width:22px;height:22px;margin-top:-3px;background:var(--p-now);border-color:#ffe0a8;box-shadow:0 0 0 5px rgba(245,181,68,.14)}
.p-vstep.gate .p-vnode{border-radius:3px;transform:rotate(45deg)}
.p-vstep.gate .p-vnode span{transform:rotate(-45deg)}
.p-vstep .rail{position:absolute;top:32px;bottom:-2px;width:2px;background:#1f2833}
.p-vstep.passed .rail{background:linear-gradient(#2f9d6e,#1f4f3c)}
.p-vcard{background:var(--p-card);border:1px solid var(--p-line);border-radius:10px;margin-bottom:8px;overflow:hidden;min-width:0}
.p-vhead{display:grid;grid-template-columns:1fr auto;gap:12px;align-items:center;padding:9px 14px;cursor:pointer;user-select:none}
.p-vhead:hover{background:#172028}
.p-vhead .t1{display:flex;align-items:center;gap:9px;flex-wrap:wrap}
.p-vhead .role{font-size:13.5px;font-weight:600}
.p-vhead .ver{font-family:var(--mono);font-size:10.5px;color:var(--p-dim2)}
.p-vhead .t2{display:flex;align-items:center;gap:10px}
.p-chip{font-size:10.5px;color:var(--p-dim);border:1px solid var(--p-line);border-radius:5px;padding:1px 7px;white-space:nowrap}
.p-chip.good{color:var(--p-pass);border-color:rgba(62,207,142,.3)}
.p-chip.hot{color:var(--p-now);border-color:rgba(245,181,68,.3)}
.p-scorechip{display:inline-flex;align-items:baseline;gap:5px;font-size:14px;font-weight:700;padding:2px 9px;
  border-radius:6px;border:1px solid;color:var(--p-now);border-color:rgba(245,181,68,.35);background:rgba(245,181,68,.08)}
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
.p-oq{border-left:2px solid var(--p-now);padding:4px 0 4px 10px;background:rgba(245,181,68,.05);
  font-size:12px;color:#e8dcc4;margin-bottom:6px}
.p-kv{display:grid;grid-template-columns:130px 1fr;gap:4px 12px;font-size:12px}
.p-kv .k{color:var(--p-dim2)}
.p-kv .v{color:#d3dce6}
.p-sec{margin-top:12px;font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2)}
.p-exec{display:flex;gap:12px;font-size:11.5px;color:#c3ccd6;padding:2px 0;font-variant-numeric:tabular-nums}

.p-hero{display:flex;align-items:center;gap:22px;border:1px solid var(--p-line);border-radius:10px;
  padding:12px 16px;background:linear-gradient(90deg, rgba(245,181,68,.06), transparent 70%)}
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
.qs2{font-size:9.5px;color:var(--p-now);border:1px solid rgba(245,181,68,.3);border-radius:4px;padding:0 4px}
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
.anchor.on{color:var(--p-ink);border-left:2px solid var(--p-now);padding-left:6px;margin-left:-8px;background:rgba(245,181,68,.07)}
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
</style>
