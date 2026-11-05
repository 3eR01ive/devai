<script setup lang="ts">
// Проекты (REQ-UI-03): project.list — корень навигации проект→задача→детали.
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { api, toUiError } from "../api";
import { fmtTime, shortId } from "../settings";
import type { Project, UiError } from "../types";

const router = useRouter();
const projects = ref<Project[]>([]);
const error = ref<UiError | null>(null);
const loading = ref(true);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    projects.value = await api.projectsList();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

function openTasks(p: Project) {
  router.push({ path: "/tasks", query: { project: p.id, name: p.name } });
}
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Проекты</h2>
      <div class="spacer" />
      <button @click="load">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <div v-if="!loading && projects.length === 0" class="empty">
      В открытом home нет проектов — add через CLI (<span class="mono">devai project add</span>)
    </div>
    <div v-for="p in projects" :key="p.id" class="panel">
      <div class="body">
        <div class="rowflex">
          <b>{{ p.name }}</b>
          <span class="mono muted">{{ shortId(p.id) }}</span>
          <div class="spacer" />
          <button @click="openTasks(p)">Задачи проекта</button>
        </div>
        <p class="muted" style="margin: 6px 0">{{ p.description || "—" }}</p>
        <p class="mono muted" style="margin: 0">
          repos: {{ p.repos.length ? p.repos.join(" · ") : "—" }}
          · обновлён {{ fmtTime(p.updated_at) }}
        </p>
      </div>
    </div>
  </section>
</template>
