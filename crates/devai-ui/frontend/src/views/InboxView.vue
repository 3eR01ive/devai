<script setup lang="ts">
// Инбокс вопросов (REQ-UI-09): где линия ждёт оператора — WAITING-задачи
// home и их OPEN-вопросы, без захода в каждую задачу. Действие (ответ)
// остаётся в CLI — OQ-UI-6 ратифицировано: база read-only.
import { onMounted, onUnmounted, ref } from "vue";
import { api, toUiError } from "../api";
import { fmtTime, pollMs, setPollMs, POLL_CHOICES, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import { type InboxEntry, type UiError } from "../types";

const entries = ref<InboxEntry[]>([]);
const error = ref<UiError | null>(null);
const interval = ref(pollMs());
let timer: number | undefined;

async function load() {
  try {
    entries.value = await api.inboxView();
    error.value = null;
  } catch (e) {
    error.value = toUiError(e);
  }
}

function applyInterval(ms: number) {
  setPollMs(ms);
  if (timer) window.clearInterval(timer);
  timer = window.setInterval(load, ms);
}

onMounted(async () => {
  await load();
  applyInterval(interval.value);
});
onUnmounted(() => {
  if (timer) window.clearInterval(timer);
});
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Вопросы к оператору</h2>
      <div class="spacer" />
      <label class="muted">опрос каждые
        <select v-model.number="interval" @change="applyInterval(interval)">
          <option v-for="ms in POLL_CHOICES" :key="ms" :value="ms">{{ ms / 1000 }} с</option>
        </select>
      </label>
      <button @click="load">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <div v-if="!entries.length" class="empty">Линия нигде не ждёт оператора — открытых вопросов нет</div>
    <div v-for="en in entries" :key="en.task.id" class="panel">
      <h3>
        <RouterLink class="title-link" :to="`/task/${en.task.id}`">{{ en.task.display }}</RouterLink>
        · {{ shortId(en.task.id) }}
      </h3>
      <div class="body">
        <div class="rowflex" style="margin-bottom: 6px">
          <StateChip :token="en.task.state" />
          <span class="muted mono">обновлена {{ fmtTime(en.task.updated_at) }}</span>
        </div>
        <div v-for="q in en.open_questions" :key="q.id" style="margin-bottom: 10px">
          <div class="rowflex">
            <span v-if="q.blocking" class="mono" style="color: var(--orange); font-weight: 700">● блокирует</span>
            <span v-else class="muted mono">○ не блокирует</span>
          </div>
          <p style="margin: 4px 0; white-space: pre-wrap">{{ q.text }}</p>
          <div class="rowflex" v-if="q.options.length">
            <span v-for="o in q.options" :key="o" class="mono" style="border: 1px solid var(--panel-border); border-radius: 4px; padding: 1px 8px; font-size: 12px">{{ o }}</span>
          </div>
        </div>
        <p v-if="!en.open_questions.length" class="muted" style="margin: 0">Задача в WAITING_FOR_INPUT, но открытых вопросов реестр не показывает.</p>
      </div>
    </div>
    <p class="muted">Ответить: <span class="mono">devai question answer &lt;id&gt; "&lt;текст&gt;"</span> — GUI этой итерации только показывает (OQ-UI-6).</p>
  </section>
</template>
