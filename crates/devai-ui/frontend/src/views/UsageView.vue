<script setup lang="ts">
// Экран расхода токенов (task 01a1b851, REQ-USG-08). ОДИН снимок
// (`usage.overview`) отдаёт сутки, месяц, топ-задачи, прогноз «на сколько
// хватит» и строку суточного гейта. Окно ничего не складывает и не
// пересчитывает: все суммы и число прогонов приходят из операции чтения
// (REQ-USG-03 AC4). «Неизвестно» показывается числом прогонов, а не нулём.
import { onMounted, ref } from "vue";
import { api, toUiError } from "../api";
import type { UiError, UsageBucketView, UsageOverviewView } from "../types";

const snap = ref<UsageOverviewView | null>(null);
const error = ref<UiError | null>(null);
const busy = ref(false);

async function load() {
  busy.value = true;
  try {
    snap.value = await api.usageOverview();
    error.value = null;
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    busy.value = false;
  }
}

/** Компактная запись числа токенов — ТОЛЬКО для чтения глазами. */
function tok(n: number | null | undefined) {
  if (n == null) return "—";
  if (n >= 1e9) return (n / 1e9).toFixed(2) + " млрд";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " млн";
  if (n >= 1e3) return Math.round(n / 1e3) + " тыс";
  return String(n);
}

function money(v: number, measured: number) {
  if (v > 0) return "$" + v.toFixed(2);
  return measured > 0 ? "неизвестна" : "—";
}

function nameOf(b: UsageBucketView) {
  return b.name ?? (b.key || "всего");
}

onMounted(load);
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Расход токенов</h2>
      <span v-if="snap" class="mono muted">
        лимиты: прогон {{ snap.limits.run ?? "без лимита" }} · задача
        {{ snap.limits.task ?? "без лимита" }} · сутки {{ snap.limits.day ?? "без лимита" }}
      </span>
      <div class="spacer" />
      <button :disabled="busy" @click="load">Обновить</button>
    </div>

    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>

    <template v-if="snap">
      <!-- Суточный гейт: одна строка с цифрами (ratified OQ-1), не вопрос на каждую задачу. -->
      <div v-if="snap.day_gate" class="errorbox">{{ snap.day_gate }}</div>

      <div class="rowflex" style="gap: 24px; align-items: flex-start">
        <div>
          <h3 style="margin: 0 0 6px">Сутки</h3>
          <div class="mono">{{ tok(snap.day.totals.total_tokens) }} токенов</div>
          <div class="muted">
            вход {{ tok(snap.day.totals.prompt_tokens) }} · выход
            {{ tok(snap.day.totals.completion_tokens) }} ·
            {{ money(snap.day.totals.cost_usd, snap.day.totals.measured) }}
          </div>
          <div class="muted">
            прогонов {{ snap.day.totals.runs }} · расход неизвестен у
            {{ snap.day.totals.unknown }}
            <template v-if="snap.day.totals.cost_unknown > 0">
              · без цены {{ snap.day.totals.cost_unknown }}
            </template>
          </div>
        </div>
        <div>
          <h3 style="margin: 0 0 6px">Месяц</h3>
          <div class="mono">{{ tok(snap.month.totals.total_tokens) }} токенов</div>
          <div class="muted">
            вход {{ tok(snap.month.totals.prompt_tokens) }} · выход
            {{ tok(snap.month.totals.completion_tokens) }} ·
            {{ money(snap.month.totals.cost_usd, snap.month.totals.measured) }}
          </div>
          <div class="muted">
            прогонов {{ snap.month.totals.runs }} · расход неизвестен у
            {{ snap.month.totals.unknown }}
          </div>
        </div>
      </div>

      <!-- Прогноз: оценка с честной оговоркой; «данных мало» — не вымысел. -->
      <p class="muted" style="margin: 12px 0">
        <b>{{ snap.forecast.available ? "Прогноз" : "Прогноз недоступен" }}:</b>
        {{ snap.forecast.line }}
        <span class="mono muted">
          (прогонов с известным расходом за сутки: {{ snap.forecast.runs_used }}, порог
          {{ snap.forecast.needed_runs }})
        </span>
      </p>

      <h3>Топ-задачи по расходу за месяц</h3>
      <table v-if="snap.top_tasks.length" class="data">
        <thead>
          <tr>
            <th>Задача</th>
            <th>Токенов</th>
            <th>Стоимость</th>
            <th>Прогонов</th>
            <th>Неизвестно</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in snap.top_tasks" :key="t.key">
            <td data-label="Задача">
              <RouterLink :to="`/proto/task/${t.key}`" class="mono">{{ nameOf(t) }}</RouterLink>
            </td>
            <td data-label="Токенов" class="mono">{{ tok(t.total_tokens) }}</td>
            <td data-label="Стоимость" class="mono">{{ money(t.cost_usd, t.measured) }}</td>
            <td data-label="Прогонов" class="mono">{{ t.runs }}</td>
            <td data-label="Неизвестно" class="mono">{{ t.unknown }}</td>
          </tr>
        </tbody>
      </table>
      <p v-else class="muted">За текущий месяц прогонов не было — расход считать не из чего.</p>
    </template>
  </section>
</template>
