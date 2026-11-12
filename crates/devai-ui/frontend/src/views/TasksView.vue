<script setup lang="ts">
// Список задач — совмещённый взгляд всех включённых домов (REQ-HOME-04):
// task-блоки home.tasks_all несут атрибуцию дома; порядок внутри блока
// отдаёт операция (обновление), UI ничего не сортирует и не схлопывает
// одноимённые задачи разных домов. Фильтр: по умолчанию все включённые
// дома, по запросу — один (home в query). Детали читаются только из
// активного дома — ссылка на чужую задачу несёт home, а экран деталей
// показывает гейт и предлагает активацию этого дома явным кликом
// (ратифицировано OQ-HOME-2/5; сам список ничего не активирует).
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, toUiError } from "../api";
import {
  blockIssueCode,
  blockIssueText,
  homeAttribution,
  homeFilterOptions,
  rowsFor,
  taskDetailLink,
  tasksEmptyText,
  tasksForProject,
} from "../homes-view";
import { fmtTime, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import RoleTimeline from "../components/RoleTimeline.vue";
import type { GraphStage } from "../types";
import type { HomeRecordsView, Task, UiError } from "../types";

const route = useRoute();
const router = useRouter();
const blocks = ref<HomeRecordsView<Task>[]>([]);
/** Путь задачи для строки списка: один `task.progress` на незакрытую
 *  задачу, ограниченной очередью — приходит готовым полем graph.stages. */
const stages = ref<Record<string, GraphStage[]>>({});
const TERMINAL = new Set(["ACCEPTED", "CANCELLED"]);
const error = ref<UiError | null>(null);
const loading = ref(true);

const homeFilter = computed(() => (route.query.home as string) ?? null);
const projectFilter = computed(() => (route.query.project as string) ?? null);
const projectName = computed(
  () => (route.query.name as string) ?? projectFilter.value,
);

async function setFilter(v: string) {
  const q = { ...route.query };
  if (v) q.home = v;
  else {
    delete q.home;
    delete q.project;
    delete q.name;
  }
  await router.replace({ path: "/tasks", query: q });
  // R3 quality return: re-read the merged view for the chosen home; the
  // query change alone left the previously loaded blocks on screen.
  await load();
}

const visible = computed(() =>
  tasksForProject(blocks.value, projectFilter.value),
);

const rows = (b: HomeRecordsView<Task>) =>
  rowsFor(visible.value, b.home_path);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    blocks.value = await api.tasksAll(homeFilter.value);
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    loading.value = false;
  }
  await loadPaths();
}

/** Пять параллельных чтений: список должен показать путь, а не ждать по
 *  одной задаче. Ошибка одной строки путь не рисует, но список не валит. */
async function loadPaths() {
  const tasks = visible.value.flatMap((b) => rows(b));
  const want = tasks.filter((t) => !TERMINAL.has(t.state)).map((t) => t.id);
  const next: Record<string, GraphStage[]> = {};
  const queue = want.slice();
  const workers = Array.from({ length: 5 }, async () => {
    while (queue.length) {
      const id = queue.shift()!;
      try {
        const p = await api.taskProgress(id);
        next[id] = p.graph?.stages ?? [];
      } catch {
        /* строка останется без пути — это честнее выдуманного */
      }
    }
  });
  await Promise.all(workers);
  stages.value = next;
}

onMounted(load);

function taskLink(b: HomeRecordsView<Task>, t: Task) {
  return taskDetailLink(b, t);
}
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">
        Задачи<span v-if="projectName" class="muted"> · проект {{ projectName }}</span>
      </h2>
      <div class="spacer" />
      <label class="muted">
        дом
        <select :value="homeFilter ?? ''" @change="setFilter(($event.target as HTMLSelectElement).value)">
          <option value="">все включённые</option>
          <option
            v-for="o in homeFilterOptions(blocks)"
            :key="'f-' + o.value"
            :value="o.value"
          >
            {{ o.label }}
          </option>
        </select>
      </label>
      <RouterLink v-if="projectFilter && !homeFilter" to="/tasks">сбросить проект</RouterLink>
      <button @click="load">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <div v-if="!loading && blocks.length === 0" class="empty">
      Нет включённых домов в реестре — добавьте дом на вкладке «Дома».
    </div>

    <div v-for="b in blocks" :key="b.home_path" style="margin-bottom: 18px">
      <div class="rowflex" style="margin-bottom: 6px">
        <b>{{ b.home_project }}</b>
        <StateChip v-if="b.is_active" token="ACTIVE" />
        <span class="mono muted" style="font-size: 12px">{{ b.home_path }}</span>
        <div class="spacer" />
        <span v-if="b.version_note" class="muted" style="font-size: 12px">{{ b.version_note }}</span>
        <span class="muted mono" style="font-size: 12px">снимок {{ fmtTime(b.snapshot_at) }}</span>
      </div>
      <div v-if="!b.available" class="errorbox">
        <span class="code">{{ blockIssueCode(b) }}</span
        >{{ blockIssueText(b) || "дом недоступен" }}
      </div>
      <div v-else-if="rows(b).length === 0" class="empty" style="padding: 8px 12px">
        {{ tasksEmptyText(projectFilter) }}
      </div>
      <table v-else class="data">
        <thead>
          <tr>
            <th>Задача</th>
            <th>Путь</th>
            <th>id</th>
            <th>Дом</th>
            <th>Состояние</th>
            <th>Обновлена</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in rows(b)" :key="b.home_path + ':' + t.id">
            <td data-label="Задача" data-mobile="title">
              <RouterLink class="title-link" :to="taskLink(b, t)">{{ t.display }}</RouterLink>
            </td>
            <td data-label="Путь">
              <RoleTimeline
                v-if="stages[t.id]?.length"
                :stages="stages[t.id]"
                :state="t.state"
              />
              <span v-else class="muted" style="font-size: 11px">
                {{ TERMINAL.has(t.state) ? "закрыта" : "…" }}
              </span>
            </td>
            <td class="mono muted" data-label="id">{{ shortId(t.id) }}</td>
            <td class="muted" :title="b.home_path" data-label="Дом">
              <!-- REQ-HOME-04 AC1 (R7 quality return): карточка несёт и имя
                   дома, и путь — видимым текстом, не только в title. -->
              <div>{{ homeAttribution(b) }}</div>
              <div class="mono" style="font-size: 12px">{{ b.home_path }}</div>
            </td>
            <td data-label="Состояние" data-mobile="head">
              <StateChip :token="t.state" />
              <StateChip v-if="t.return_state" :token="t.return_state" />
            </td>
            <td class="mono muted" data-label="Обновлена" data-mobile="head">{{ fmtTime(t.updated_at) }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>
