<script setup lang="ts">
// Вклад «качество» карточки задачи (приёмка): общий балл крупно, вклад
// каждого пункта, отметки «в доработку / снято с контроля».
//
// Данные — ТОЛЬКО из сохранённого отчёта качества (`report.report`):
// evaluation.checks (балл, вес, состояние, свидетельство) и
// human_view.headline (итог, порог, вердикт). Окно ничего не пересчитывает
// как правило платформы: вклад = балл × вес считается здесь только для
// показа, а итог берётся готовым из headline.
import { computed, ref } from "vue";
import type { QualityReportView } from "../types";

const props = defineProps<{ report: QualityReportView | null }>();

type Check = {
  id: string;
  weight: number | null;
  score: number | null;
  state: string;
  evidence?: string[];
  violations?: string[];
  rule?: {
    group?: string;
    statement?: string;
    verify?: string;
    anchors?: Record<string, string>;
    floor?: number | null;
    detects?: { t: string; sev?: string | null }[];
  };
};

const payload = computed<Record<string, unknown>>(
  () => (props.report?.report as Record<string, unknown>) ?? {},
);
const evaluation = computed<Record<string, unknown>>(
  () => (payload.value.evaluation as Record<string, unknown>) ?? {},
);
const checks = computed<Check[]>(
  () => ((evaluation.value.checks as Check[]) ?? []).slice(),
);
const headline = computed<Record<string, unknown>>(
  () =>
    ((payload.value.human_view as Record<string, unknown>)
      ?.headline as Record<string, unknown>) ?? {},
);
const profile = computed<Record<string, unknown>>(
  () => (payload.value.profile as Record<string, unknown>) ?? {},
);

const score = computed<number | null>(() =>
  typeof headline.value.score === "number" ? (headline.value.score as number) : null,
);
const theta = computed<number | null>(() =>
  typeof headline.value.theta === "number" ? (headline.value.theta as number) : null,
);
const verdict = computed<string>(() => String(headline.value.verdict ?? ""));

/** Цвет главного показателя: окончательный вердикт и порог решают, а не вкус. */
const scoreTone = computed(() => {
  if (score.value === null) return "none";
  if (theta.value !== null && score.value < theta.value) return "bad";
  const v = verdict.value;
  if (v && !v.includes("НЕ ГОТОВ")) return "ok";
  return "wait";
});

const totalWeight = computed(() =>
  checks.value.reduce((s, c) => s + (c.weight ?? 0), 0),
);
const scoredWeight = computed(() =>
  checks.value.reduce((s, c) => s + (c.score === null ? 0 : (c.weight ?? 0)), 0),
);
const contribSum = computed(() =>
  checks.value.reduce((s, c) => s + (c.score ?? 0) * (c.weight ?? 0), 0),
);
const lossSum = computed(() =>
  checks.value.reduce(
    (s, c) => s + (c.score === null ? 0 : (1 - (c.score ?? 0)) * (c.weight ?? 0)),
    0,
  ),
);
const unscored = computed(() => checks.value.filter((c) => c.score === null).length);
const perfect = computed(() => checks.value.filter((c) => c.score === 1).length);
const mid = computed(
  () => checks.value.filter((c) => c.score !== null && c.score > 0 && c.score < 1 && c.score >= 0.6).length,
);
const low = computed(
  () => checks.value.filter((c) => c.score !== null && c.score > 0 && c.score < 0.6).length,
);

// ── отметки: по умолчанию всё идёт в доработку; снятая отметка — «снято с контроля»
const picked = ref<Set<string>>(new Set(checks.value.map((c) => c.id)));
const initPick = () => {
  if (picked.value.size === 0) picked.value = new Set(checks.value.map((c) => c.id));
};
initPick();

const mode = ref<"loss" | "all">("loss");
const openId = ref<string | null>(null);

function togglePick(id: string) {
  const next = new Set(picked.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  picked.value = next;
}

function toggleOpen(id: string) {
  openId.value = openId.value === id ? null : id;
}

const rows = computed(() => {
  let list = checks.value.slice();
  if (mode.value === "loss") {
    list = list.filter((c) => c.score === null || c.score < 1);
    list.sort((a, b) => loss(b) - loss(a) || (b.weight ?? 0) - (a.weight ?? 0));
  } else {
    list.sort((a, b) => (b.weight ?? 0) - (a.weight ?? 0) || a.id.localeCompare(b.id));
  }
  return list;
});

const maxWeight = computed(() =>
  rows.value.reduce((m, c) => Math.max(m, c.weight ?? 0), 0) || 1,
);

const loss = (c: Check) =>
  c.score === null ? -1 : (1 - (c.score ?? 0)) * (c.weight ?? 0);
const contrib = (c: Check) => (c.score === null ? null : (c.score ?? 0) * (c.weight ?? 0));

const tone = (c: Check) =>
  c.score === null ? "un" : c.score >= 1 ? "ok" : c.score >= 0.6 ? "mid" : "low";

const pct = (v: number | null) => (v === null ? "—" : (v * 100).toFixed(1));
const num = (v: number | null) =>
  v === null ? "—" : v >= 0.01 ? v.toFixed(3) : v.toFixed(4);
const evidenceText = (c: Check) => (c.violations?.[0] ?? c.evidence?.[0] ?? "");
const anchorKeys = (c: Check) =>
  ["1.0", "0.7", "0.4", "0.0"].filter((k) => c.rule?.anchors?.[k] != null);

const markedCount = computed(() => picked.value.size);
const unmarkedCount = computed(() => checks.value.length - picked.value.size);
</script>

<template>
  <div v-if="!report" class="empty">отчёта качества ещё нет — судить нечего</div>
  <div v-else class="qc">
    <!-- главный показатель -->
    <div class="hero" :class="scoreTone">
      <div class="big">
        {{ pct(score) }}<span class="p">%</span>
      </div>
      <div class="side">
        <div class="lab">общая оценка качества</div>
        <div class="track">
          <i :style="{ width: (score === null ? 0 : Math.min(100, score * 100)) + '%' }" />
          <b :style="{ left: (theta === null ? 80 : Math.min(100, theta * 100)) + '%' }" />
        </div>
        <div class="sub">
          порог {{ pct(theta) }}%
          <template v-if="score !== null && theta !== null">
            · {{ score >= theta ? "выше на" : "ниже на" }}
            {{ Math.abs((score - theta) * 100).toFixed(1) }} пп
          </template>
          <template v-if="verdict"> · {{ verdict.toLowerCase() }}</template>
        </div>
      </div>
    </div>

    <div class="cells">
      <div class="cell">
        <div class="k">балл</div>
        <div class="v">{{ score === null ? "—" : score.toFixed(3) }}<span>/1.00</span></div>
        <div class="n">профиль v{{ profile.version ?? "—" }}</div>
      </div>
      <div class="cell">
        <div class="k">сумма весов</div>
        <div class="v">{{ num(totalWeight) }}</div>
        <div class="n">оценено {{ num(scoredWeight) }} · не оценено {{ unscored }}</div>
      </div>
      <div class="cell">
        <div class="k">пунктов</div>
        <div class="v">{{ checks.length }}</div>
        <div class="n">{{ perfect }} по 1.0 · {{ mid }} средних · {{ low }} слабых</div>
      </div>
      <div class="cell">
        <div class="k">сумма вклада</div>
        <div class="v">{{ num(contribSum) }}</div>
        <div class="n">потеряно {{ num(lossSum) }}</div>
      </div>
    </div>

    <!-- формула -->
    <details class="formula">
      <summary>как из этих цифр получается балл</summary>
      <div class="f">
        каждая проверка ставит <b>балл 0…1</b> и имеет <b>вес</b>. Вклад = балл × вес.
        Итоговый балл = сумма вкладов / сумма весов <b>оценённых</b> проверок:
        непроверенные выпадают, веса нормируются заново. Вердикт считается по порядку:
        шлюз не пройден → возврат; шлюз не выполнен, нет подписи или вес блокеров выше
        предела → «не решено»; пробита красная линия (пол) или балл ниже порога →
        возврат; иначе приёмка.
      </div>
    </details>

    <!-- инструменты -->
    <div class="tools">
      <button class="tab" :class="{ on: mode === 'loss' }" @click="mode = 'loss'">
        только потери · {{ checks.filter((c) => c.score === null || c.score < 1).length }}
      </button>
      <button class="tab" :class="{ on: mode === 'all' }" @click="mode = 'all'">
        все · {{ checks.length }}
      </button>
      <span class="pickbar">
        на доработку <b>{{ markedCount }}</b> · снято с контроля <b>{{ unmarkedCount }}</b>
      </span>
      <span class="hint">снятая отметка = пункт не держит вердикт</span>
    </div>

    <div class="head">
      <span /><span>пункт</span><span>важность</span><span>балл</span>
      <span>вклад</span><span>потеря</span>
    </div>

    <div v-if="rows.length === 0" class="empty">потерь нет: все пункты на 1.0</div>

    <div v-for="c in rows" :key="c.id" class="item" :class="[tone(c), { picked: picked.has(c.id), open: openId === c.id }]">
      <div class="row" @click="toggleOpen(c.id)">
        <button
          class="pick"
          :aria-checked="picked.has(c.id)"
          :title="picked.has(c.id) ? 'снять с контроля' : 'отправить на доработку'"
          @click.stop="togglePick(c.id)"
        >
          {{ picked.has(c.id) ? "✕" : "" }}
        </button>
        <div class="id">
          <span class="caret">▶</span>{{ c.id }}
          <span v-if="c.state && c.state !== 'SCORED'" class="st">{{ c.state }}</span>
          <span v-if="!picked.has(c.id)" class="free">снято с контроля</span>
        </div>
        <div class="bar">
          <em><i :style="{ width: Math.max((100 * (c.weight ?? 0)) / maxWeight, 1.5) + '%' }" /></em>
          <span>{{ num(c.weight) }}</span>
        </div>
        <div class="score">{{ c.score === null ? "—" : c.score.toFixed(1) }}</div>
        <div class="contrib">{{ num(contrib(c)) }}</div>
        <div class="loss">{{ c.score === null ? "—" : loss(c) > 0 ? "−" + loss(c).toFixed(3) : "0" }}</div>
      </div>

      <div v-if="openId === c.id" class="body">
        <div v-if="c.rule" class="rule">
          <div class="rh">
            <span class="g">{{ c.rule.group }}</span>
            <span>проверка: {{ c.rule.verify || "—" }}</span>
            <span>вес {{ num(c.weight) }}</span>
            <span v-if="c.rule.floor != null" class="floor">пол {{ c.rule.floor }}</span>
          </div>
          <div class="rs">{{ c.rule.statement }}</div>
          <div v-if="anchorKeys(c).length" class="anchors">
            <div
              v-for="k in anchorKeys(c)"
              :key="k"
              class="anchor"
              :class="{ on: c.score !== null && Number(k) === Number(c.score) }"
            >
              <span class="v">{{ k }}</span><span class="t">{{ c.rule.anchors?.[k] }}</span>
            </div>
          </div>
          <div v-if="c.rule.detects?.length" class="det">
            ловит: {{ c.rule.detects.map((d) => d.t + (d.sev ? ` (${d.sev})` : "")).join(", ") }}
          </div>
        </div>
        <div class="ev">{{ evidenceText(c) }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.qc{display:flex;flex-direction:column;gap:10px}
.empty{color:var(--slate);font-size:12px}
.hero{display:flex;align-items:center;gap:22px;border:1px solid var(--panel-border);
  background:var(--panel);border-radius:10px;padding:14px 18px}
.big{font-size:54px;font-weight:700;letter-spacing:-2px;line-height:1;color:var(--slate);
  font-variant-numeric:tabular-nums}
.hero.ok .big{color:#7bbf9a}
.hero.wait .big{color:var(--orange)}
.hero.bad .big{color:#e2606b}
.p{font-size:24px;color:var(--slate);margin-left:3px}
.side{flex:1}
.lab{font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--slate)}
.track{position:relative;height:10px;border-radius:5px;background:rgba(113,128,141,.22);margin:8px 0 6px;max-width:520px}
.track i{position:absolute;left:0;top:0;bottom:0;border-radius:5px;background:#7bbf9a}
.hero.wait .track i{background:var(--orange)}
.hero.bad .track i{background:#e2606b}
.track b{position:absolute;top:-3px;width:2px;height:16px;background:var(--cream);opacity:.7}
.sub{font-size:11px;color:var(--slate)}
.cells{display:grid;grid-template-columns:repeat(4,1fr);gap:8px}
.cell{border:1px solid var(--panel-border);background:rgba(17,24,32,.45);border-radius:8px;padding:9px 11px}
.k{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--slate)}
.v{font-size:19px;font-weight:650;margin-top:3px;font-variant-numeric:tabular-nums}
.v span{font-size:11px;color:var(--slate)}
.n{font-size:10.5px;color:var(--slate);margin-top:2px}
.formula summary{color:var(--orange);font-size:11px;cursor:pointer}
.f{font-size:11.5px;color:#c8d0d8;margin-top:6px;line-height:1.55;
  border:1px solid var(--panel-border);border-radius:8px;padding:9px 11px;background:rgba(17,24,32,.45)}
.f b{color:var(--cream)}
.tools{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.tab{font:inherit;font-size:11px;padding:3px 10px;border-radius:6px;cursor:pointer;
  border:1px solid var(--panel-border);background:transparent;color:var(--slate)}
.tab.on{color:var(--cream);background:rgba(113,128,141,.18);border-color:var(--slate)}
.pickbar{font-size:11px;color:var(--slate)}
.pickbar b{color:var(--cream);font-variant-numeric:tabular-nums}
.hint{font-size:10.5px;color:var(--slate);margin-left:auto}
.head,.row{display:grid;grid-template-columns:24px 1fr 150px 52px 64px 64px;gap:8px;align-items:center}
.head{font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--slate);padding:0 2px}
.item{border-bottom:1px solid rgba(113,128,141,.18);border-left:2px solid transparent}
.item.picked{border-left-color:#e2606b;background:rgba(226,96,107,.05)}
.item:not(.picked){opacity:.72}
.row{padding:6px 2px;font-size:12px;cursor:pointer;font-variant-numeric:tabular-nums}
.row:hover .id{color:var(--cream)}
.pick{width:18px;height:18px;padding:0;border-radius:4px;border:1px solid var(--panel-border);
  background:rgba(17,24,32,.6);color:#e2606b;font-size:11px;cursor:pointer;line-height:1}
.item.picked .pick{border-color:#e2606b;background:rgba(226,96,107,.16)}
.id{font-family:var(--mono);font-size:11.5px;color:#cfd8e2;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}
.caret{display:inline-block;width:12px;color:var(--slate);font-size:9px}
.item.open .caret{color:var(--cream)}
.st{font-size:9.5px;color:var(--orange);border:1px solid rgba(243,106,22,.35);border-radius:4px;padding:0 4px}
.free{font-size:9.5px;color:var(--slate);border:1px dashed var(--panel-border);border-radius:4px;padding:0 5px;margin-left:6px}
.bar{display:flex;align-items:center;gap:7px}
.bar em{position:relative;flex:1;height:7px;border-radius:4px;background:rgba(113,128,141,.22);font-style:normal}
.bar em i{position:absolute;left:0;top:0;bottom:0;border-radius:4px;background:#6d8ba6}
.item.ok .bar em i{background:#2f7d5c}
.item.low .bar em i{background:#b0475a}
.item.mid .bar em i{background:#a8802f}
.bar span{font-size:10.5px;color:var(--slate);min-width:34px;text-align:right}
.score{color:#cfd8e2}
.item.ok .score{color:#7bbf9a}
.item.mid .score{color:var(--orange)}
.item.low .score{color:#e2606b}
.contrib{color:#cfd8e2}
.loss{color:#e2606b}
.item.ok .loss{color:var(--slate)}
.body{padding:2px 0 8px 14px;border-left:2px solid rgba(113,128,141,.25);margin:2px 0 6px 2px}
.rule{border:1px solid var(--panel-border);border-radius:8px;padding:9px 11px;background:rgba(17,24,32,.5);margin-bottom:8px}
.rh{display:flex;gap:10px;flex-wrap:wrap;font-size:10.5px;color:var(--slate)}
.rh .g{color:var(--orange)}
.floor{color:#e2606b}
.rs{font-size:12px;color:#d7dee5;line-height:1.5;margin-top:5px;max-width:130ch}
.anchors{margin-top:7px;display:flex;flex-direction:column;gap:2px}
.anchor{display:flex;gap:8px;font-size:11px;color:var(--slate)}
.anchor.on{color:var(--cream);border-left:2px solid var(--orange);padding-left:6px;margin-left:-8px;
  background:rgba(243,106,22,.07)}
.anchor .v{font-family:var(--mono);min-width:26px}
.det{margin-top:5px;font-size:10.5px;color:var(--slate)}
.ev{font-size:11.5px;color:#a9b6c2;line-height:1.5;white-space:pre-wrap;max-width:140ch}

@media (max-width: 767px) {
  .anchor, .bar span, .caret, .det, .ev, .f, .formula summary, .free, .head, .hint, .id,
  .k, .lab, .n, .pick, .pickbar, .rh, .st, .sub, .tab, .v span { font-size: 12px; }
}
</style>
