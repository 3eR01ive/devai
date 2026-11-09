<script setup lang="ts">
// Раздел «Связка модели» (REQ-MS-07, REQ-MC-01..09): действующая связка, ОДНО
// слово статуса с задержкой и временем пробы, объявленные связки, история,
// баннер отказов и идущие прогоны — всё из ОДНОГО снимка (`model_snapshot`).
// Окно не ходит в стор и не собирает экран из нескольких операций.
//
// Авторская поверхность (создание/правка/удаление) — те же операции платформы,
// что видит консоль: сохранение всегда делает СВОЮ пробу текущих значений,
// кнопка «Проверить» — отдельный настоящий вызов, который ничего не пишет.
// Удаление — удержание на самой кнопке: обработчика click нет, поэтому
// одиночный и двойной клик ничего не удаляют. Действующую связку удалить
// нельзя — управление подписано причиной.
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { api, toUiError } from "../api";
import {
  bundleIdFromLabel,
  HOLD_TIMER_GRACE_MS,
  HOLD_TO_SWITCH_MS,
  holdCompletes,
} from "../model_view";
import { fmtTime } from "../settings";
import type {
  BundleFormInput,
  BundleMutationView,
  ModelBundleView,
  ModelSnapshot,
  SwitchResultView,
  UiError,
} from "../types";

const snap = ref<ModelSnapshot | null>(null);
const error = ref<UiError | null>(null);
const notice = ref<SwitchResultView | null>(null);
const mutation = ref<BundleMutationView | null>(null);
const probeLine = ref<Record<string, string>>({});
const holding = ref<string | null>(null);
const holdProgress = ref(0);
let holdTimer: ReturnType<typeof setTimeout> | undefined;
let progressTimer: ReturnType<typeof setInterval> | undefined;
let holdStart = 0;

const status = computed(() => snap.value?.status ?? null);

// --- форма (создание/правка) -----------------------------------------
interface FormState {
  label: string;
  agent: string;
  driver: string;
  llm_provider: string;
  model: string;
  reasoning: string;
  probe_phrase: string;
  probe_timeout_ms: number;
  fresh_secs: number;
}

const formOpen = ref(false);
/** id правимой связки; null — форма создания (id выводится из имени). */
const editing = ref<string | null>(null);
const form = ref<FormState>(blankForm());
const formError = ref<UiError | null>(null);
const formProbe = ref<string | null>(null);
const formBusy = ref(false);
/** Редакция файла, на которой открыта форма (REQ-MC-06 AC6). */
const formRevision = ref("");

const drivers = computed(() => snap.value?.form?.drivers ?? []);
const selfProviderDriver = computed(
  () => snap.value?.form?.self_provider_driver ?? "claude",
);
/** Идентификатор показывается ДО сохранения; у правки он неизменяем. */
const formId = computed(() => editing.value ?? bundleIdFromLabel(form.value.label));
const providerLocked = computed(
  () => form.value.driver === selfProviderDriver.value,
);

function blankForm(): FormState {
  const f = snap.value?.form;
  return {
    label: "",
    agent: "",
    driver: "",
    llm_provider: "",
    model: "",
    reasoning: "",
    probe_phrase: f?.default_phrase ?? "ping",
    probe_timeout_ms: f?.default_timeout_ms ?? 20000,
    fresh_secs: f?.default_fresh_secs ?? 900,
  };
}

function snapshotRevision(): string {
  return snap.value?.file_revision ?? "";
}

function openCreate() {
  editing.value = null;
  form.value = blankForm();
  formError.value = null;
  formProbe.value = null;
  formRevision.value = snapshotRevision();
  formOpen.value = true;
}

function openEdit(b: ModelBundleView) {
  editing.value = b.id;
  form.value = {
    label: b.label,
    agent: b.agent,
    driver: b.driver,
    llm_provider: b.llm_provider,
    model: b.model,
    reasoning: b.reasoning,
    probe_phrase: b.probe_phrase,
    probe_timeout_ms: b.probe_timeout_ms,
    fresh_secs: b.fresh_secs,
  };
  formError.value = null;
  formProbe.value = null;
  formRevision.value = snapshotRevision();
  formOpen.value = true;
}

function closeForm() {
  formOpen.value = false;
  formError.value = null;
  formProbe.value = null;
}

function payload(): BundleFormInput {
  return {
    id: editing.value ?? "",
    label: form.value.label,
    agent: form.value.agent,
    driver: form.value.driver,
    llm_provider: providerLocked.value ? "" : form.value.llm_provider,
    model: form.value.model,
    reasoning: form.value.reasoning,
    probe_phrase: form.value.probe_phrase,
    probe_timeout_ms: Number(form.value.probe_timeout_ms),
    fresh_secs: Number(form.value.fresh_secs),
    editing: editing.value !== null,
    file_revision: formRevision.value,
  };
}

async function check() {
  formError.value = null;
  formProbe.value = null;
  formBusy.value = true;
  try {
    const p = await api.modelBundleCheck(payload());
    formProbe.value =
      p.verdict === "ok"
        ? `проверка ok · ${p.latency_ms} мс`
        : `проверка ${p.verdict} (${p.verdict_text})`;
    await load();
  } catch (e) {
    formError.value = toUiError(e);
    await load();
  } finally {
    formBusy.value = false;
  }
}

async function save() {
  formError.value = null;
  mutation.value = null;
  formBusy.value = true;
  try {
    const input = payload();
    mutation.value = editing.value
      ? await api.modelBundleUpdate(input)
      : await api.modelBundleCreate(input);
    formOpen.value = false;
    await load();
  } catch (e) {
    formError.value = toUiError(e);
    // The refusal is a stored fact (REQ-MC-07): reload so the history shows
    // it without a window restart. The form keeps its own file revision.
    await load();
  } finally {
    formBusy.value = false;
  }
}

async function applyDelete(id: string) {
  error.value = null;
  mutation.value = null;
  try {
    mutation.value = await api.modelBundleDelete(id, snapshotRevision());
    await load();
  } catch (e) {
    error.value = toUiError(e);
    await load();
  }
}

// ---------------------------------------------------------------

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

function holdKey(kind: "switch" | "delete", id: string): string {
  return kind === "delete" ? `del:${id}` : `sw:${id}`;
}

function startHold(kind: "switch" | "delete", id: string) {
  cancelHold();
  holding.value = holdKey(kind, id);
  holdStart = performance.now();
  // Удержание — единственный путь применения: таймер заканчивается ровно на
  // пороге, а прогресс только заливает кнопку. Обработчика click нет.
  progressTimer = setInterval(() => {
    holdProgress.value = Math.min(1, (performance.now() - holdStart) / HOLD_TO_SWITCH_MS);
  }, 40);
  holdTimer = setTimeout(() => {
    const completes =
      holding.value === holdKey(kind, id) &&
      holdCompletes(performance.now() - holdStart);
    // Clear the timers UNCONDITIONALLY before applying: a hold that ends
    // without completing (or a test that outlived it) must not leave the
    // progress interval running — a live interval keeps the process awake.
    cancelHold();
    if (completes) {
      if (kind === "delete") {
        void applyDelete(id);
      } else {
        void applySwitch(id);
      }
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

// Провайдер не принимается у драйвера, который выбирает его сам (REQ-MC-08):
// поле очищается вместе с переключением драйвера, а не отклоняется потом.
watch(
  () => form.value.driver,
  (driver) => {
    if (driver === selfProviderDriver.value) form.value.llm_provider = "";
  },
);

onMounted(load);
onUnmounted(cancelHold);
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Связка модели</h2>
      <span v-if="snap" class="mono muted">снимок {{ fmtTime(snap.snapshot_at) }}</span>
      <div class="spacer" />
      <button @click="openCreate">Создать связку</button>
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
    <div v-if="mutation" class="errorbox applied" data-testid="model-mutation">
      <span class="code">{{ mutation.outcome }}</span>
      {{ mutation.label }} ({{ mutation.bundle_id }}) — {{ mutation.outcome_text }}
      <template v-if="mutation.verdict"> · проба {{ mutation.verdict }} {{ mutation.latency_ms }} мс</template>
      <template v-if="mutation.agent_changed">
        · смена имени агента — идущие прогоны остаются на прежнем
      </template>
    </div>

    <!-- Форма создания/правки (REQ-MC-01/03/08): сохранение — только после
         собственной успешной пробы; никаких ключей и адресов сервиса. -->
    <div v-if="formOpen" class="panel" data-testid="model-form">
      <div class="rowflex">
        <b>{{ editing ? `Правка связки ${editing}` : "Новая связка" }}</b>
        <div class="spacer" />
        <span class="mono muted">идентификатор: {{ formId || "—" }}</span>
      </div>
      <div class="muted">
        Идентификатор выводится из имени; после сохранения он не меняется.
        Сохранение всегда проверяет введённые значения настоящим вызовом и пишет
        файл только при успехе.
      </div>

      <div class="formgrid">
        <label>Имя связки<input v-model="form.label" data-field="label" /></label>
        <label>Агент<input v-model="form.agent" data-field="agent" /></label>
        <label>Драйвер
          <input v-model="form.driver" list="model-drivers" data-field="driver" />
          <datalist id="model-drivers">
            <option v-for="d in drivers" :key="d" :value="d" />
          </datalist>
        </label>
        <label>LLM-провайдер
          <input
            v-model="form.llm_provider"
            :disabled="providerLocked"
            data-field="llm_provider"
          />
          <span v-if="providerLocked" class="muted">
            драйвер {{ form.driver }} выбирает провайдера сам — поле пустое
          </span>
        </label>
        <label>Модель
          <input v-model="form.model" data-field="model" />
          <span class="muted">при создании модель обязательна</span>
        </label>
        <label>Уровень размышления
          <input v-model="form.reasoning" data-field="reasoning" />
          <span class="muted">обязателен: уровень — часть связки</span>
        </label>
        <label>Фраза пробы
          <input v-model="form.probe_phrase" data-field="probe_phrase" />
        </label>
        <label>Таймаут пробы, мс
          <input v-model.number="form.probe_timeout_ms" type="number" data-field="probe_timeout_ms" />
        </label>
        <label>Срок свежести пробы, с
          <input v-model.number="form.fresh_secs" type="number" data-field="fresh_secs" />
        </label>
      </div>

      <div class="muted">
        Драйверы: {{ drivers.join(", ") }}. Диагностический драйвер mock в списке
        не предлагается — у него нет сетевой проверки. Ключи и адрес сервиса в
        связке не хранятся.
      </div>

      <div v-if="formError" class="errorbox">
        <span class="code">{{ formError.code }}</span>{{ formError.message }}
      </div>
      <div v-if="formProbe" class="mono muted">{{ formProbe }}</div>

      <div class="rowflex">
        <button :disabled="formBusy" @click="check">Проверить</button>
        <button :disabled="formBusy" @click="save">Сохранить (с пробой)</button>
        <button @click="closeForm">Отмена</button>
      </div>
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
            <th>Правка</th>
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
                :style="{ '--fill': `${(holding === `sw:${b.id}` ? holdProgress : 0) * 100}%` }"
                @pointerdown="startHold('switch', b.id)"
                @pointerup="cancelHold"
                @pointerleave="cancelHold"
                @pointercancel="cancelHold"
              >
                {{ b.active ? "Переключено" : "Удержать: переключить" }}
              </button>
            </td>
            <td>
              <button @click="openEdit(b)">Править</button>
              <div v-if="b.active" class="muted" data-testid="delete-locked">
                действующая — сначала переключитесь
              </div>
              <button
                v-else
                class="hold"
                data-testid="delete-hold"
                :style="{ '--fill': `${(holding === `del:${b.id}` ? holdProgress : 0) * 100}%` }"
                @pointerdown="startHold('delete', b.id)"
                @pointerup="cancelHold"
                @pointerleave="cancelHold"
                @pointercancel="cancelHold"
              >
                Удержать: удалить
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
          <template v-if="e.outcome"> · <span class="mono">{{ e.outcome_text ?? e.outcome }}</span></template>
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
.formgrid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 6px 12px;
  margin: 8px 0;
}
.formgrid label {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
</style>
