<script setup lang="ts">
// Раздел «Связка модели» (REQ-MS-07): действующая связка, ОДНО слово
// статуса с задержкой и временем пробы, объявленные связки, история,
// баннер отказов и идущие прогоны — всё из ОДНОГО снимка (`model_snapshot`).
// Окно не ходит в стор и не собирает экран из нескольких операций.
//
// Переключение — удержание на самой кнопке (без модального подтверждения):
// обработчика click нет вообще, поэтому одиночный и двойной клик ничего не
// применяют. Перед применением виден вердикт пробы: кнопка «Проба» пишет
// вердикт рядом. Промах не применяется — платформа отказывает типизированно.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { api, toUiError } from "../api";
import {
  HOLD_TIMER_GRACE_MS,
  HOLD_TO_SWITCH_MS,
  holdCompletes,
} from "../model_view";
import { fmtTime } from "../settings";
import type { ModelSnapshot, SwitchResultView, UiError } from "../types";

const snap = ref<ModelSnapshot | null>(null);
const error = ref<UiError | null>(null);
const notice = ref<SwitchResultView | null>(null);
const probeLine = ref<Record<string, string>>({});
const holding = ref<string | null>(null);
const holdProgress = ref(0);
let holdTimer: ReturnType<typeof setTimeout> | undefined;
let progressTimer: ReturnType<typeof setInterval> | undefined;
let holdStart = 0;

const status = computed(() => snap.value?.status ?? null);

async function load() {
  try {
    snap.value = await api.modelSnapshot();
    error.value = null;
  } catch (e) {
    error.value = toUiError(e);
  }
}

async function probe(id: string) {
  error.value = null;
  notice.value = null;
  try {
    const p = await api.modelProbe(id);
    probeLine.value = {
      ...probeLine.value,
      [id]:
        p.verdict === "ok"
          ? `проба ok · ${p.latency_ms} мс`
          : `проба ${p.verdict} (${p.verdict_text})`,
    };
    await load();
  } catch (e) {
    error.value = toUiError(e);
  }
}

async function applySwitch(id: string) {
  error.value = null;
  try {
    notice.value = await api.modelSwitch(id, "manual");
    await load();
  } catch (e) {
    error.value = toUiError(e);
    await load();
  }
}

function startHold(id: string) {
  cancelHold();
  holding.value = id;
  holdStart = performance.now();
  // Удержание — единственный путь применения: таймер заканчивается ровно на
  // пороге, а прогресс только заливает кнопку. Обработчика click нет.
  progressTimer = setInterval(() => {
    holdProgress.value = Math.min(1, (performance.now() - holdStart) / HOLD_TO_SWITCH_MS);
  }, 40);
  holdTimer = setTimeout(() => {
    const completes =
      holding.value === id && holdCompletes(performance.now() - holdStart);
    // Clear the timers UNCONDITIONALLY before applying: a hold that ends
    // without completing (or a test that outlived it) must not leave the
    // progress interval running — a live interval keeps the process awake.
    cancelHold();
    if (completes) {
      void applySwitch(id);
    }
  }, HOLD_TO_SWITCH_MS + HOLD_TIMER_GRACE_MS);
}

function cancelHold() {
  if (holdTimer) clearTimeout(holdTimer);
  if (progressTimer) clearInterval(progressTimer);
  holdTimer = undefined;
  progressTimer = undefined;
  holding.value = null;
  holdProgress.value = 0;
}

onMounted(load);
onUnmounted(cancelHold);
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Связка модели</h2>
      <span v-if="snap" class="mono muted">снимок {{ fmtTime(snap.snapshot_at) }}</span>
      <div class="spacer" />
      <button @click="load">Обновить</button>
    </div>

    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <div v-if="notice" class="errorbox" :class="notice.outcome">
      <span class="code">{{ notice.code }}</span>
      <template v-if="notice.outcome === 'applied'">
        переключено на {{ notice.bundle_id }} — проба ok, {{ notice.latency_ms }} мс
        <template v-if="notice.agent_changed">
          · идущие прогоны остаются на прежнем агенте<template v-if="snap?.running.length">:
            <b>{{ snap.running.map((r) => r.agent ?? "—").join(", ") }}</b></template>
        </template>
      </template>
      <template v-else>
        не переключено: проба {{ notice.verdict }} — запись и отметка не менялись
      </template>
    </div>

    <template v-if="snap">
      <!-- Действующая связка + ОДНО поле статуса. -->
      <div class="panel">
        <div class="rowflex">
          <b>{{ snap.active ? (snap.active.label ?? "(не объявлена в конфиге)") : "связка не выбрана" }}</b>
          <span v-if="snap.active" class="mono muted">{{ snap.active.id ?? "—" }}</span>
          <div class="spacer" />
          <b data-testid="model-status">{{ status?.state_text ?? "неизвестно" }}</b>
          <span v-if="status?.latency_ms != null" class="mono muted">{{ status.latency_ms }} мс</span>
          <span class="mono muted">{{ status?.freshness_text ?? "доказательств нет" }}</span>
        </div>
        <div v-if="snap.active" class="muted">
          агент <span class="mono">{{ snap.active.agent }}</span> ·
          драйвер <span class="mono">{{ snap.active.driver }}</span> ·
          LLM-провайдер <span class="mono">{{ snap.active.llm_provider_text }}</span> ·
          модель <span class="mono">{{ snap.active.model_text }}</span> ·
          уровень размышления <span class="mono">{{ snap.active.reasoning_text }}</span>
        </div>
        <div v-if="status?.reason" class="muted">
          причина: {{ status.reason_text ?? "причина не распознана" }}
          <span v-if="status.probe_older"> · проба старее проваленной строки исполнения</span>
        </div>
        <div v-if="status?.evidence_at" class="muted">
          доказательство: {{ status.evidence }} · {{ fmtTime(status.evidence_at) }}
        </div>
      </div>

      <!-- Баннер отказов: горит только при «не работает», само не применяется. -->
      <div v-if="snap.failures" class="errorbox" data-testid="model-banner">
        <span class="code">{{ snap.failures.code }}</span>{{ snap.failures.text }}
        <template v-if="snap.failures.suggestion">
          — предложение: <b>{{ snap.failures.suggestion }}</b> (удержать «Переключить» на её строке)
        </template>
        <template v-else> — живой связки со свежей пробой нет, предложения нет</template>
      </div>

      <!-- Объявленные связки целиком: без прокрутки во всплывашках и без «+N ещё». -->
      <table class="data">
        <thead>
          <tr>
            <th>Связка</th><th>Агент</th><th>Драйвер</th><th>LLM-провайдер</th>
            <th>Модель</th><th>Уровень размышления</th><th>Проба</th><th>Применение</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="b in snap.declared" :key="b.id" :class="{ active: b.active }">
            <td>
              <b>{{ b.label }}</b>
              <div class="mono muted">{{ b.id }}</div>
            </td>
            <td class="mono">{{ b.agent }}</td>
            <td class="mono">{{ b.driver }}</td>
            <td class="mono">{{ b.llm_provider_text }}</td>
            <td class="mono">{{ b.model_text }}</td>
            <td class="mono">{{ b.reasoning_text }}</td>
            <td>
              <button @click="probe(b.id)">Проба</button>
              <div v-if="probeLine[b.id]" class="mono muted">{{ probeLine[b.id] }}</div>
            </td>
            <td>
              <button
                class="hold"
                :style="{ '--fill': `${(holding === b.id ? holdProgress : 0) * 100}%` }"
                @pointerdown="startHold(b.id)"
                @pointerup="cancelHold"
                @pointerleave="cancelHold"
                @pointercancel="cancelHold"
              >
                {{ b.active ? "Переключено" : "Удержать: переключить" }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <div v-if="snap.running.length" class="panel">
        <b>Идущие прогоны</b>
        <div v-for="r in snap.running" :key="r.execution_id" class="muted">
          {{ r.task_id }} · агент <span class="mono">{{ r.agent ?? "—" }}</span> · с {{ fmtTime(r.started_at) }}
        </div>
        <div class="muted">
          При смене ИМЕНИ агента идущие прогоны остаются на прежнем до конца; при смене модели
          того же агента новая связка подхватывается следующей стадией.
        </div>
      </div>

      <div class="panel">
        <b>История</b>
        <div v-for="(e, i) in snap.history" :key="i" class="muted">
          {{ fmtTime(e.at) }} · <span class="mono">{{ e.kind }}</span> · {{ e.actor }} ·
          {{ e.bundle_id ?? "—" }}
          <template v-if="e.verdict"> · вердикт <span class="mono">{{ e.verdict }}</span></template>
          <template v-if="e.outcome"> · <span class="mono">{{ e.outcome }}</span></template>
          <template v-if="e.reason"> · причина {{ e.reason_text ?? "причина не распознана" }}</template>
          <template v-if="e.agent_changed"> · смена имени агента</template>
        </div>
      </div>
    </template>
  </section>
</template>

<style scoped>
.panel {
  border: 1px solid var(--border);
  padding: 8px 10px;
  margin-bottom: 10px;
}
tr.active td {
  background: var(--orange-dim, rgba(255, 140, 0, 0.08));
}
button.hold {
  background: linear-gradient(
    to right,
    var(--orange) var(--fill, 0%),
    transparent var(--fill, 0%)
  );
  touch-action: none;
}
</style>
