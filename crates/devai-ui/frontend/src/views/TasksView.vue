<script setup lang="ts">
// Список задач (REQ-UI-04): task.list отдаёт порядок по обновлению — UI
// ничего не сортирует сам. Токен Task.state печатается буквально.
// Фильтр по проекту (REQ-UI-03 AC2) — проекция данных task.project_id.
import { computed, onMounted, ref } from "vue";
import { useRoute } from "vue-router";
import { api, toUiError } from "../api";
import { fmtTime, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import { type Task, type UiError } from "../types";

const route = useRoute();
const tasks = ref<Task[]>([]);
const error = ref<UiError | null>(null);
const loading = ref(true);

const projectFilter = computed(() => (route.query.project as string) ?? null);
const projectName = computed(() => (route.query.name as string) ?? projectFilter.value);

const visible = computed(() =>
  projectFilter.value
    ? tasks.value.filter((t) => t.project_id === projectFilter.value)
    : tasks.value,
);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    tasks.value = await api.tasksList();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">
        Задачи<span v-if="projectName" class="muted"> · проект {{ projectName }}</span>
      </h2>
      <div class="spacer" />
      <RouterLink v-if="projectFilter" to="/tasks">все задачи home</RouterLink>
      <button @click="load">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <div v-if="!loading && visible.length === 0" class="empty">
      {{ projectFilter ? "У проекта пока нет задач" : "Home без задач — пустое состояние, не ошибка" }}
    </div>
    <table v-if="visible.length" class="data">
      <thead>
        <tr>
          <th>Задача</th>
          <th>id</th>
          <th>Состояние</th>
          <th>Обновлена</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="t in visible" :key="t.id">
          <td>
            <RouterLink class="title-link" :to="`/task/${t.id}`">{{ t.display }}</RouterLink>
          </td>
          <td class="mono muted">{{ shortId(t.id) }}</td>
          <td>
            <StateChip :token="t.state" />
            <StateChip v-if="t.return_state" :token="t.return_state" />
          </td>
          <td class="mono muted">{{ fmtTime(t.updated_at) }}</td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
