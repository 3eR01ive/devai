<script setup lang="ts">
// Мини-график сходимости стадии (REQ-CV-10): инлайн-SVG без зависимостей,
// без интерактива и без сглаживания. Компонент РИСУЕТ готовые значения:
// балл/порог/объём/режимы/причины приходят посчитанными платформой
// (task.convergence для страницы задачи, факт inbox.snapshot для строки
// «Ожидания») — здесь нет ни пересчёта балла, ни вывода режима.
//
// Ось X — круг, ось Y — метрика: score_milli/1000 у quality, требования
// (сплошная) и открытые вопросы (пунктир) у spec_review. Линия порога —
// только quality и только по СОХРАНЁННОМУ theta каждого круга; theta нет —
// линии нет (0.80 не подставляется). Неизмеренный круг и граница
// profile-сегмента — разрыв линии, не ноль.
import { computed } from "vue";
import type { ChartPoint } from "../convergence_view";
import {
  lastRoundLabel,
  missingLabel,
  modeLabel,
  oqLine,
  plot,
} from "../convergence_view";

const props = withDefaults(
  defineProps<{
    stage: string;
    series: ChartPoint[];
    gapMilli: number;
    modes: string[];
    modeNote: string | null;
    missing: string[];
    /** Страница задачи рвёт линию на границе профиля; строка «Ожидания» — нет
     *  (её серия по контракту не несёт profile). */
    breakOnProfile?: boolean;
  }>(),
  { breakOnProfile: false },
);

const isQuality = computed(() => props.stage === "quality");
const drawing = computed(() => plot(props.series, props.stage, props.breakOnProfile));

const polyline = (idx: number[]): string =>
  idx
    .map((i) => `${drawing.value.points[i].x.toFixed(1)},${drawing.value.points[i].y.toFixed(1)}`)
    .join(" ");

const dotClass = (modes: string[]): string => {
  if (modes.includes("DIVERGING")) return "dot diverging";
  if (modes.includes("STUCK")) return "dot stuck";
  if (modes.includes("PLATEAU")) return "dot plateau";
  if (modes.includes("CONVERGING")) return "dot converging";
  return "dot plain";
};

const oqPoints = computed(() => {
  const top = Math.max(1, ...props.series.map((p) => p.requirements_count ?? 0));
  return oqLine(props.series, top)
    .map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`)
    .join(" ");
});

const lastProfile = computed(() => {
  const last = props.series[props.series.length - 1];
  return last?.profile ?? null;
});

const headline = computed(() => lastRoundLabel(props.modes, props.modeNote));
</script>

<template>
  <div class="conv">
    <p class="conv-head">
      <span class="mono stage">{{ stage === "quality" ? "QA / реворк" : "спека / ревью" }}</span>
      <span class="muted mono">{{ series.length }} круг(ов)</span>
      <span v-if="lastProfile" class="muted mono">профиль {{ lastProfile }}</span>
    </p>

    <p v-if="!series.length" class="muted" style="margin: 0">кругов пока нет</p>

    <template v-else>
      <svg
        class="conv-chart"
        :viewBox="`0 0 ${drawing.width} ${drawing.height}`"
        role="img"
        :aria-label="`сходимость стадии ${stage}`"
      >
        <!-- Оси: X — круги, Y — метрика. Никаких обработчиков: график читают глазами. -->
        <line
          :x1="8"
          :y1="drawing.height - 16"
          :x2="drawing.width - 8"
          :y2="drawing.height - 16"
          class="axis"
        />
        <!-- Линия порога (только quality, только сохранённый theta). -->
        <line
          v-for="(t, i) in drawing.threshold"
          :key="`t${i}`"
          :x1="t.x1"
          :y1="t.y1"
          :x2="t.x2"
          :y2="t.y2"
          class="threshold"
        />
        <!-- Требования (сплошная) и открытые вопросы (пунктир) стадии spec. -->
        <polyline
          v-for="(seg, i) in drawing.segments"
          :key="`s${i}`"
          :points="polyline(seg)"
          class="line"
        />
        <polyline v-if="!isQuality && oqPoints" :points="oqPoints" class="line oq" />
        <circle
          v-for="p in drawing.points.filter((x) => x.measured)"
          :key="`p${p.round}`"
          :cx="p.x"
          :cy="p.y"
          r="3"
          :class="dotClass(p.modes)"
        />
        <!-- Неизмеренный круг: пустая метка на оси, не ноль. -->
        <g v-for="p in drawing.points.filter((x) => !x.measured)" :key="`m${p.round}`">
          <circle :cx="p.x" :cy="drawing.height - 16" r="2.5" class="gap-dot" />
          <text :x="p.x" :y="drawing.height - 5" text-anchor="middle" class="axis-label">
            {{ p.round }}
          </text>
        </g>
      </svg>

      <!-- Подпись последнего круга: все режимы по порядку платформы, иначе — причина. -->
      <p class="conv-last">
        <span class="mono">круг {{ drawing.points[drawing.points.length - 1].round }}</span>
        <span v-if="headline" :class="modes.length ? 'mode' : 'muted'">{{ headline }}</span>
        <span v-if="series.length === 1" class="muted">· данных мало — один круг</span>
      </p>
      <p v-if="missing.length" class="muted mono conv-gaps">
        пробелы: {{ missing.map(missingLabel).join(" · ") }}
      </p>
      <p v-if="modes.length" class="muted conv-gaps">
        режимы последнего круга: {{ modes.map(modeLabel).join(" + ") }}
      </p>
    </template>
  </div>
</template>

<style scoped>
.conv-head {
  margin: 0 0 4px;
  display: flex;
  gap: 10px;
  align-items: baseline;
}
.conv-head .stage {
  color: var(--orange);
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.conv-chart {
  width: 100%;
  max-width: 360px;
  height: auto;
  display: block;
}
.axis {
  stroke: var(--panel-border);
  stroke-width: 1;
}
.axis-label {
  fill: var(--slate);
  font-family: var(--mono);
  font-size: 9px;
}
.line {
  fill: none;
  stroke: var(--slate);
  stroke-width: 1.5;
}
.line.oq {
  stroke-dasharray: 3 3;
  opacity: 0.8;
}
.threshold {
  stroke: var(--orange);
  stroke-width: 1;
  stroke-dasharray: 5 3;
  opacity: 0.7;
}
.dot.plain {
  fill: var(--slate);
}
.dot.converging {
  fill: #2f9e6d;
}
.dot.diverging {
  fill: #d64545;
}
.dot.plateau {
  fill: #d6a545;
}
.dot.stuck {
  fill: #9b59b6;
}
.gap-dot {
  fill: none;
  stroke: var(--slate);
  stroke-width: 1;
}
.conv-last {
  margin: 2px 0 0;
  display: flex;
  gap: 8px;
  align-items: baseline;
}
.conv-last .mode {
  font-weight: 700;
  color: var(--orange);
  font-size: 12px;
}
.conv-gaps {
  margin: 2px 0 0;
  font-size: 11px;
}
</style>