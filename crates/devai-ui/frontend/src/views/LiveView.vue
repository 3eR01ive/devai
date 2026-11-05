<script setup lang="ts">
// Живые исполнения (REQ-UI-10): task.active — все RUNNING-строки всех
// задач одновременно, порядок oldest-first делает API. Интервал поллинга —
// настройка UI (localStorage, A-UI-5), snapshot_at — платформа.
import { onMounted, onUnmounted, ref } from "vue";
import { api, toUiError } from "../api";
import { fmtElapsed, fmtTime, pollMs, setPollMs, POLL_CHOICES, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import type { ActiveSnapshot, UiError } from "../types";

const snap = ref<ActiveSnapshot | null>(null);
const error = ref<UiError | null>(null);
const interval = ref(pollMs());
let timer: number | undefined;

async function load() {
  try {
    snap.value = await api.taskActive();
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
      <h2 style="margin: 0">Активные исполнения</h2>
      <span v-if="snap" class="mono muted">снимок {{ fmtTime(snap.snapshot_at) }}</span>
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
    <div v-if="snap && !snap.rows.length" class="empty">Сейчас нет RUNNING-исполнений</div>
    <table v-if="snap && snap.rows.length" class="data">
      <thead>
        <tr>
          <th>Задача</th><th>Роль@вер</th><th>Агент</th><th>Состояние</th>
          <th>Шёл</th><th>Owner</th><th>Live</th><th>Stale-признаки</th><th>Worktree</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="r in snap.rows" :key="r.id">
          <td><RouterLink class="title-link" :to="`/task/${r.task_id}`">{{ shortId(r.task_id) }}</RouterLink></td>
          <td class="mono">{{ r.role }}</td>
          <td>{{ r.agent_name ?? "—" }}</td>
          <td><StateChip :token="r.state" /></td>
          <td class="mono muted">{{ fmtElapsed(r.elapsed_seconds) }}</td>
          <td class="mono muted">{{ r.owner_pid ?? "?" }}@{{ r.owner_host ?? "?" }}</td>
          <td><StateChip :token="r.owner_liveness" /></td>
          <td class="mono muted">
            <template v-if="r.stale">
              {{ r.stale_terminal_task ? "task-terminal" : "" }}
              {{ r.stale_dead_owner ? "dead-owner" : "" }}
            </template>
            <template v-else>—</template>
          </td>
          <td class="mono muted">
            <template v-if="r.branch || r.worktree">{{ r.branch ?? "" }}<br v-if="r.worktree" />{{ r.worktree ?? "" }}</template>
            <template v-else>—</template>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
