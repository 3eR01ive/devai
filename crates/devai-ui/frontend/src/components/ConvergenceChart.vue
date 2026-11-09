<script setup lang="ts">
// Мини-график сходимости стадии (REQ-CV-13): инлайн-SVG без зависимостей,
// без интерактива и без сглаживания. Компонент РИСУЕТ готовые значения:
// балл, порог, объём и режим приходят посчитанными платформой
// (`task.progress` для страницы задачи, `inbox.snapshot` для строки
// «Ожидания») — здесь нет ни пересчёта балла, ни вывода режима.
//
// Ось X — круг, ось Y — метрика: score_milli/1000 у quality, ДВЕ счётные
// линии (требования и открытые вопросы) на общей оси «штуки» у
// spec_review. Линия порога — только quality и только по СОХРАНЁННОМУ
// theta каждого круга; theta нет — линии нет и есть подпись «порог
// неизвестен». Неизмеренный круг, null-счётчик и граница профиля рвут
// линию, а не рисуются нулём.
import { computed } from "vue";
import type { ChartPoint } from "../convergence_view";
import {
  basisLabel,
  gapsText,
  missingLabel,
  scoreText,
  volumeLabel,
  plot,
} from "../convergence_view";

const props = withDefaults(
  defineProps<{
    stage: string;
    series: ChartPoint[];
    gapMilli: number;
    /** Страница задачи рвёт линию на границе профиля; строка «Ожидания»
     *  получает тот же объект, поэтому тоже рвёт — флаг оставлен для
     *  явности двух поверхностей. */
    breakOnProfile?: boolean;
  }>(),
  { breakOnProfile: true },
);

const isQuality = computed(() => props.stage === "quality");
const drawing = computed(() => plot(props.series, props.stage, props.breakOnProfile));
const last = computed(() => props.series[props.series.length - 1] ?? null);

const polyline = (idx: number[]): string =>
  idx
    .map((i) => `${drawing.value.points[i].x.toFixed(1)},${drawing.value.points[i].y.toFixed(1)}`)
    .join(" ");

const dotClass = (mode: string): string => {
  if (mode === "DIVERGING") return "dot diverging";
  if (mode === "STUCK") return "dot stuck";
  if (mode === "PLATEAU") return "dot plateau";
  if (mode === "CONVERGING") return "dot converging";
  return "dot plain";
};

const lastProfile = computed(() => last.value?.profile ?? null);
const headline = computed(() =>
  last.value ? basisLabel(last.value.mode, last.value.mode_basis) : "",
);
const thetaUnknown = computed(
  () => isQuality.value && last.value !== null && last.value.theta_milli === null,
);
const readErrorText = computed(() => {
  const err = last.value?.read_error ?? null;
  return err ? `сбой чтения: ${err.code} — ${err.message}` : "";
});
const specVolume = computed(() =>
  isQuality.value ? "" : volumeLabel(last.value?.volume_direction ?? null),
);

/**
 * Счётчики стадии (REQ-CV-13): «не измерено» — круги без точки,
 * «ждут» — проверки, ждущие подписи человека, «дефект» — нечисловые
 * баллы. Это суммы готовых полей платформы, без пересчёта балла.
 */
const counters = computed(() => {
  let unmeasured = 0;
  let waiting = 0;
  let defect = 0;
  for (const p of props.series) {
    if (!p.measured) unmeasured += 1;
    waiting += p.pending_count ?? 0;
    defect += p.invalid_count ?? 0;
  }
  return { unmeasured, waiting, defect };
});

/** Подпись последнего круга: готовое значение, без арифметики. */
const lastValue = computed(() => {
  if (!last.value) return "";
  if (isQuality.value) {
    return `S=${scoreText(last.value.score_milli)} · пунктов ниже 1.0: ${
      last.value.low_count ?? "—"
    }`;
  }
  return `требований ${last.value.requirements_count ?? "—"} · открытых вопросов ${
    last.value.oq_count ?? "—"
  }`;
});
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
        <!-- quality: одна линия балла; spec: требования (сплошная) + OQ (пунктир). -->
        <polyline
          v-for="(seg, i) in drawing.scoreSegments"
          :key="`s${i}`"
          :points="polyline(seg)"
          class="line"
        />
        <polyline
          v-for="(seg, i) in drawing.reqSegments"
          :key="`r${i}`"
          :points="polyline(seg)"
          class="line req"
        />
        <polyline
          v-for="(seg, i) in drawing.oqSegments"
          :key="`o${i}`"
          :points="polyline(seg)"
          class="line oq"
        />
        <circle
          v-for="p in drawing.points.filter((x) => x.measured)"
          :key="`p${p.round}`"
          :cx="p.x"
          :cy="p.y"
          r="3"
          :class="dotClass(p.mode)"
        />
        <!-- Неизмеренный круг: пустая метка на оси, не ноль. -->
        <g v-for="p in drawing.points.filter((x) => !x.measured)" :key="`m${p.round}`">
          <circle :cx="p.x" :cy="drawing.height - 16" r="2.5" class="gap-dot" />
          <text :x="p.x" :y="drawing.height - 5" text-anchor="middle" class="axis-label">
            {{ p.round }}
          </text>
        </g>
      </svg>

      <!-- Бейдж последнего круга: mode + mode_basis (REQ-CV-13). -->
      <p class="conv-last">
        <span class="mono">круг {{ drawing.points[drawing.points.length - 1].round }}</span>
        <span class="mode" :class="last?.mode === 'UNKNOWN' ? 'unknown' : ''">{{ headline }}</span>
      </p>
      <p class="muted mono conv-value">{{ lastValue }}<template v-if="specVolume"> · {{ specVolume }}</template></p>
      <p class="muted mono conv-counters">
        не измерено {{ counters.unmeasured }}<template v-if="isQuality"> · ждут {{ counters.waiting }} · дефект {{ counters.defect }}</template>
      </p>
      <p v-if="thetaUnknown" class="muted mono conv-gaps">порог неизвестен</p>
      <p v-if="readErrorText" class="muted mono conv-gaps">{{ readErrorText }}</p>
      <p v-if="last?.missing.length" class="muted mono conv-gaps">
        пробелы: {{ gapsText(last.missing) }}
      </p>
      <p v-if="series.length === 1" class="muted conv-gaps">
        данных мало — один круг, режим не определён
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
.line.req {
  stroke: var(--orange);
  opacity: 0.85;
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
.conv-last .mode.unknown {
  color: var(--slate);
  font-weight: 600;
}
.conv-value {
  margin: 2px 0 0;
  font-size: 11px;
}
.conv-counters {
  margin: 0;
  font-size: 11px;
}
.conv-gaps {
  margin: 2px 0 0;
  font-size: 11px;
}
</style>
