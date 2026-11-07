<script setup lang="ts">
// Проекты — совмещённый взгляд всех включённых домов (REQ-HOME-04, форма
// утверждена OQ-HOME-6): блоки home.projects_all несут атрибуцию дома в
// каждом блоке, активный дом визуально отделён, фильтр по умолчанию — все
// включённые дома, по запросу — один. Недоступный дом виден с путём и
// причиной; его записи не выдумываются (REQ-HOME-07).
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, toUiError } from "../api";
import {
  blockIssueCode,
  blockIssueText,
  homeAttribution,
  homeFilterOptions,
} from "../homes-view";
import { fmtTime, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import type { HomeRecordsView, Project, UiError } from "../types";

const route = useRoute();
const router = useRouter();
const blocks = ref<HomeRecordsView<Project>[]>([]);
const error = ref<UiError | null>(null);
const loading = ref(true);

// Фильтр-дом живёт в query — переключается без рестарта и переживает
// перезагрузку окна.
const homeFilter = ref<string | null>((route.query.home as string) ?? null);

async function setFilter(v: string) {
  homeFilter.value = v === "" ? null : v;
  const q = { ...route.query };
  if (homeFilter.value) q.home = homeFilter.value;
  else delete q.home;
  await router.replace({ path: "/projects", query: q });
  // R3 quality return: the select must actually re-read the merged view —
  // previously only the query changed and the rows stayed of the old home.
  await load();
}

async function load() {
  loading.value = true;
  error.value = null;
  try {
    blocks.value = await api.projectsAll(homeFilter.value);
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

function openTasks(b: HomeRecordsView<Project>, p: Project) {
  router.push({
    path: "/tasks",
    query: { home: b.home_path, project: p.id, name: p.name },
  });
}
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Проекты</h2>
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
      <RouterLink to="/homes">реестр домов</RouterLink>
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
      <div
        v-else-if="b.records.length === 0"
        class="empty"
        style="padding: 8px 12px"
      >
        В этом доме пока нет проектов
      </div>
      <div v-for="p in b.records" :key="b.home_path + ':' + p.id" class="panel">
        <div class="body">
          <div class="rowflex">
            <b>{{ p.name }}</b>
            <span class="mono muted">{{ shortId(p.id) }}</span>
            <span class="muted" style="font-size: 12px">
              · дом {{ homeAttribution(b) }} ·
              <span class="mono">{{ b.home_path }}</span>
            </span>
            <div class="spacer" />
            <button @click="openTasks(b, p)">Задачи проекта</button>
          </div>
          <p class="muted" style="margin: 6px 0">{{ p.description || "—" }}</p>
          <p class="mono muted" style="margin: 0">
            repos: {{ p.repos.length ? p.repos.join(" · ") : "—" }}
            · обновлён {{ fmtTime(p.updated_at) }}
          </p>
        </div>
      </div>
    </div>
  </section>
</template>
