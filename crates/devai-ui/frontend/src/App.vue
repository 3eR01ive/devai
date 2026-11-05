<script setup lang="ts">
// Каркас окна (REQ-UI-01): навигация + статус-бар с фактом открытого
// home — путь виден всегда (AC3). home резолвится бэкендом по ratified
// цепочке (DEVAI_HOME > ~/.devai); фронт только показывает данные сессии.
import { onMounted, ref } from "vue";
import { api, toUiError } from "./api";
import { fmtTime } from "./settings";
import type { SessionInfo, UiError } from "./types";

const session = ref<SessionInfo | null>(null);
const error = ref<UiError | null>(null);

onMounted(async () => {
  try {
    session.value = await api.sessionInfo();
  } catch (e) {
    error.value = toUiError(e);
  }
});
</script>

<template>
  <div class="shell">
    <header class="topbar">
      <span class="brand">dev<em>ai</em>-ui</span>
      <nav class="modes">
        <RouterLink to="/projects">Проекты</RouterLink>
        <RouterLink to="/tasks">Задачи</RouterLink>
        <RouterLink to="/live">Активные</RouterLink>
        <RouterLink to="/inbox">Вопросы</RouterLink>
        <RouterLink to="/ops">Операции API</RouterLink>
      </nav>
      <div class="statusbar" v-if="session">
        home <b>{{ session.home }}</b><br />
        actor <b>{{ session.actor }}</b> · снимок {{ fmtTime(session.opened_at) }}
      </div>
    </header>
    <main class="content">
      <div v-if="error" class="errorbox">
        <span class="code">{{ error.code }}</span>{{ error.message }}
      </div>
      <RouterView />
    </main>
  </div>
</template>
