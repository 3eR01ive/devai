<script setup lang="ts">
// Вертикальный путь задачи: этапы-роли и этапы-решения человека.
// Открыт всегда ровно один шаг (по умолчанию — текущий); внутри шага —
// вкладки «вход»/«выход» (что роль читает и что отдаёт) и, у приёмки,
// «качество». Кнопка запуска/остановки показывается ТОЛЬКО когда это
// разрешает платформа (`allowed_operations`) или идёт живой прогон.
import { computed, ref, watch } from "vue";
import type { QualityReportView, RoleGraph } from "../types";
import QualityChecks from "./QualityChecks.vue";

const props = defineProps<{
  graph: RoleGraph;
  allowed: string[];
  report: QualityReportView | null;
  snapshotAt?: string;
}>();

const emit = defineEmits<{
  (e: "run", roleSlug: string): void;
  (e: "stop", executionId: string): void;
}>();

type Row = {
  state: string;
  label: string;
  executor: string;
  gate: boolean;
  status: "passed" | "current" | "pending";
  status_word: string;
  role_slug: string | null;
  role_version: number | null;
  executions: { id: string; status: string; attempt: number; started_at: string; finished_at: string | null; role_display: string }[];
  artifacts: { id: string; display: string; status: string }[];
  running: number;
  can_run: boolean;
  run_op: string;
  why: string;
  score: number | null;
  score_tone: string;
};

const nodes = computed(() =>
  new Map(props.graph.nodes.map((n) => [n.state, n])),
);

const currentIdx = computed(() =>
  props.graph.stages.findIndex((s) => s.status === "current"),
);

const openState = ref<string | null>(
  currentIdx.value >= 0
    ? props.graph.stages[currentIdx.value].state
    : props.graph.stages[props.graph.stages.length - 1]?.state ?? null,
);
const openTab = ref<string>("in");

watch(
  () => props.graph.stages.map((s) => s.state + s.status).join(","),
  () => {
    const i = currentIdx.value;
    if (i >= 0) openState.value = props.graph.stages[i].state;
  },
);

const reportScore = computed<number | null>(() => {
  const body = (props.report?.report as Record<string, unknown>) ?? {};
  const hl = ((body.human_view as Record<string, unknown>)?.headline ??
    {}) as Record<string, unknown>;
  return typeof hl.score === "number" ? (hl.score as number) : null;
});
const reportTheta = computed<number | null>(() => {
  const body = (props.report?.report as Record<string, unknown>) ?? {};
  const hl = ((body.human_view as Record<string, unknown>)?.headline ??
    {}) as Record<string, unknown>;
  return typeof hl.theta === "number" ? (hl.theta as number) : null;
});
const reportVerdict = computed<string>(() => {
  const body = (props.report?.report as Record<string, unknown>) ?? {};
  const hl = ((body.human_view as Record<string, unknown>)?.headline ??
    {}) as Record<string, unknown>;
  return String(hl.verdict ?? "");
});

const rows = computed<Row[]>(() =>
  props.graph.stages.map((st) => {
    const n = nodes.value.get(st.state);
    const executions = (n?.executions ?? []).map((e) => ({
      id: e.id,
      status: String(e.status),
      attempt: e.attempt,
      started_at: e.started_at,
      finished_at: e.finished_at,
      role_display: e.role_display,
    }));
    const running = executions.filter((e) => e.status === "RUNNING").length;
    const slug = n?.role_slug ?? null;
    const runOp = slug
      ? props.allowed.find((o) => o === `run-role-execution(${slug})`) ?? ""
      : "";
    const can_run = !st.gate && (!!runOp || (slug !== null && props.allowed.includes("run")));
    let why = "";
    if (!can_run && !running) {
      why = st.gate ? "решает человек" : st.status === "passed" ? "шаг пройден" : "платформа пока не разрешает запуск";
    }
    const isQa = props.report !== null && st.gate && String(st.state) === "QA";
    const score = isQa ? reportScore.value : null;
    let score_tone = "wait";
    if (score !== null) {
      if (reportTheta.value !== null && score < reportTheta.value) score_tone = "bad";
      else if (reportVerdict.value && !reportVerdict.value.includes("НЕ ГОТОВ")) score_tone = "ok";
    }
    return {
      ...st,
      role_slug: slug,
      role_version: n?.role_version ?? null,
      executions,
      artifacts: (n?.artifacts ?? []).map((a) => ({
        id: a.id,
        display: a.display,
        status: String(a.status),
      })),
      running,
      can_run,
      run_op: runOp,
      why,
      score,
      score_tone,
    };
  }),
);

const tabsFor = (r: Row) => {
  if (r.gate) return r.score !== null ? [["qa", "качество"]] : [];
  return [
    ["in", "вход"],
    ["out", "выход"],
  ];
};

/** Открытая вкладка всегда есть у открытого шага: у приёмки это «качество». */
const syncTab = () => {
  const r = rows.value.find((x) => x.state === openState.value);
  if (!r) return;
  const tabs = tabsFor(r);
  if (!tabs.length) return;
  if (!tabs.some((t) => String(t[0]) === openTab.value)) openTab.value = String(tabs[0][0]);
};

function open(r: Row) {
  if (openState.value === r.state) {
    openState.value = null;
    return;
  }
  openState.value = r.state;
  const t = tabsFor(r);
  openTab.value = t.length ? String(t[0][0]) : "in";
}

watch([rows, openState], syncTab, { immediate: true });

const inContract = (r: Row) => (nodes.value.get(r.state)?.input_contract ?? null);
const outContract = (r: Row) => (nodes.value.get(r.state)?.output_contract ?? null);

const fmtDuration = (e: { started_at: string; finished_at: string | null }) => {
  const a = new Date(e.started_at).getTime();
  const b = e.finished_at ? new Date(e.finished_at).getTime() : Date.now();
  const s = Math.max(0, Math.round((b - a) / 1000));
  return s < 90 ? `${s} с` : s < 5400 ? `${Math.round(s / 60)} мин` : `${Math.round(s / 3600)} ч`;
};

const compact = (n: number) =>
  n >= 1e9 ? (n / 1e9).toFixed(2) + " млрд" : n >= 1e6 ? (n / 1e6).toFixed(1) + " млн" : String(n);
</script>

<template>
  <div class="steps">
    <div
      v-for="(r, i) in rows"
      :key="r.state"
      class="step"
      :class="[r.status, { gate: r.gate, open: openState === r.state }]"
    >
      <div class="nodebox">
        <div class="node"><span>{{ r.status === "passed" ? "✓" : "" }}</span></div>
        <div v-if="i < rows.length - 1" class="rail" />
      </div>

      <div class="card">
        <div class="head" @click="open(r)">
          <div class="t1">
            <span class="role">{{ r.label }}</span>
            <span v-if="r.role_slug" class="ver">{{ r.role_slug }}@v{{ r.role_version }}</span>
            <span
              v-if="r.score !== null"
              class="chip-score"
              :class="r.score_tone"
            >{{ (r.score * 100).toFixed(1) }}%<i>качество</i></span>
            <span class="chip" :class="r.status === 'passed' ? 'good' : r.status === 'current' ? 'hot' : ''">
              {{ r.status_word }}
            </span>
            <span v-if="r.executions.length" class="chip">прогонов {{ r.executions.length }}</span>
            <span v-if="r.gate" class="chip hot">решает человек</span>
          </div>
          <div class="t2">
            <button
              v-if="r.running"
              class="act stop"
              title="остановить прогон"
              @click.stop="emit('stop', r.executions.find((e) => e.status === 'RUNNING')!.id)"
            >
              ■ остановить
            </button>
            <button
              v-else-if="r.can_run"
              class="act run"
              :title="r.run_op"
              @click.stop="emit('run', r.role_slug!)"
            >
              ▶ запустить
            </button>
            <span v-else-if="r.status === 'current'" class="act why">{{ r.why }}</span>
            <span class="caret">▶</span>
          </div>
        </div>

        <div v-if="openState === r.state" class="body">
          <div v-if="tabsFor(r).length" class="tabs">
            <button
              v-for="t in tabsFor(r)"
              :key="String(t[0])"
              class="tab"
              :class="{ on: openTab === String(t[0]) }"
              @click="openTab = String(t[0])"
            >
              {{ t[1] }}
            </button>
          </div>

          <div v-if="openTab === 'in' && !r.gate" class="panel">
            <div class="kv">
              <span class="k">контракт</span>
              <span class="v">{{ inContract(r) ?? "—" }}</span>
              <span class="k">роль</span>
              <span class="v">{{ r.executions[0]?.role_display ?? r.role_slug ?? "—" }}</span>
            </div>
            <div class="sec">прогоны · {{ r.executions.length }}</div>
            <div v-if="!r.executions.length" class="muted">прогонов не было</div>
            <div v-for="e in r.executions.slice(-6)" :key="e.id" class="exec">
              <span class="mono">#{{ e.attempt }}</span>
              <span>{{ e.status }}</span>
              <span class="mono">{{ fmtDuration(e) }}</span>
            </div>
          </div>

          <div v-if="openTab === 'out' && !r.gate" class="panel">
            <div class="kv">
              <span class="k">контракт</span>
              <span class="v">{{ outContract(r) ?? "—" }}</span>
            </div>
            <div class="sec">артефакты · {{ r.artifacts.length }}</div>
            <div v-if="!r.artifacts.length" class="muted">ничего не отдавал</div>
            <div v-for="a in r.artifacts.slice(-8)" :key="a.id" class="exec">
              <span>{{ a.display }}</span><span class="mono">{{ a.status }}</span>
            </div>
          </div>

          <div v-if="openTab === 'qa'" class="panel">
            <QualityChecks :report="report" />
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.steps{display:flex;flex-direction:column;gap:2px}
.step{display:grid;grid-template-columns:40px minmax(0,1fr);gap:12px;position:relative}
.nodebox{display:flex;flex-direction:column;align-items:center;padding-top:14px;position:relative}
.node{width:16px;height:16px;border-radius:50%;border:2px solid var(--panel-border);
  background:rgba(17,24,32,.8);display:flex;align-items:center;justify-content:center;
  font-size:9px;color:var(--coal-lo);z-index:2}
.step.passed .node{background:#4f9d76;border-color:#4f9d76;color:#0f1418}
.step.current .node{width:22px;height:22px;margin-top:-3px;background:var(--orange);
  border-color:#ffb07a;box-shadow:0 0 0 5px rgba(243,106,22,.16)}
.step.gate .node{border-radius:3px;transform:rotate(45deg)}
.step.gate .node span{transform:rotate(-45deg)}
.rail{position:absolute;top:32px;bottom:-2px;width:2px;background:rgba(113,128,141,.25)}
.step.passed .rail{background:#3d7a5c}

.card{border:1px solid var(--panel-border);background:var(--panel);border-radius:10px;
  margin-bottom:8px;overflow:hidden;min-width:0}
.head{display:grid;grid-template-columns:1fr auto;gap:12px;align-items:center;
  padding:9px 14px;cursor:pointer;user-select:none}
.head:hover{background:rgba(113,128,141,.08)}
.t1{display:flex;align-items:center;gap:9px;flex-wrap:wrap}
.role{font-size:13.5px;font-weight:600}
.ver{font-family:var(--mono);font-size:10.5px;color:var(--slate)}
.chip{font-size:10.5px;color:var(--slate);border:1px solid var(--panel-border);
  border-radius:5px;padding:1px 7px;white-space:nowrap}
.chip.good{color:#7bbf9a;border-color:rgba(123,191,154,.4)}
.chip.hot{color:var(--orange);border-color:rgba(243,106,22,.4)}
.chip-score{display:inline-flex;align-items:baseline;gap:5px;font-size:14px;font-weight:700;
  padding:2px 9px;border-radius:6px;border:1px solid;font-variant-numeric:tabular-nums;line-height:1.1}
.chip-score i{font-size:9.5px;font-weight:500;font-style:normal;letter-spacing:.06em;text-transform:uppercase;opacity:.75}
.chip-score.ok{color:#7bbf9a;border-color:rgba(123,191,154,.45);background:rgba(123,191,154,.1)}
.chip-score.wait{color:var(--orange);border-color:rgba(243,106,22,.45);background:rgba(243,106,22,.1)}
.chip-score.bad{color:#e2606b;border-color:rgba(226,96,107,.45);background:rgba(226,96,107,.1)}
.t2{display:flex;align-items:center;gap:10px}
.act{font:inherit;font-size:11px;padding:3px 10px;border-radius:6px;cursor:pointer;
  border:1px solid var(--panel-border);background:transparent;color:var(--cream);white-space:nowrap}
.act.run{border-color:rgba(123,191,154,.5);color:#9fe0bd;background:rgba(123,191,154,.1)}
.act.run:hover{background:rgba(123,191,154,.2)}
.act.stop{border-color:rgba(226,96,107,.5);color:#f0a0a8;background:rgba(226,96,107,.1)}
.act.stop:hover{background:rgba(226,96,107,.2)}
.act.why{font-size:10px;color:var(--slate);border:0;background:none;cursor:default}
.caret{color:var(--slate);font-size:9px}
.step.open .caret{color:var(--cream)}
.tabs{display:flex;gap:6px;padding:8px 14px;border-bottom:1px solid var(--panel-border)}
.tab{font:inherit;font-size:11px;padding:3px 10px;border-radius:6px;cursor:pointer;
  border:1px solid var(--panel-border);background:transparent;color:var(--slate)}
.tab.on{color:var(--cream);background:rgba(113,128,141,.18);border-color:var(--slate)}
.panel{padding:12px 14px}
.kv{display:grid;grid-template-columns:130px 1fr;gap:4px 12px;font-size:12px}
.k{color:var(--slate)}
.v{color:#d7dee5}
.sec{margin-top:12px;font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--slate)}
.exec{display:flex;gap:12px;font-size:11.5px;color:#c3ccd6;padding:2px 0;font-variant-numeric:tabular-nums}
.mono{font-family:var(--mono);color:var(--slate)}
.muted{color:var(--slate);font-size:12px}

@media (max-width: 767px) {
  .act, .act.why, .caret, .chip, .chip-score i, .exec, .node, .sec, .tab, .ver { font-size: 12px; }
}
</style>
