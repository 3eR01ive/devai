<script setup lang="ts">
// Каркас окна (REQ-UI-01): навигация + статус-бар с фактом открытого
// home — путь виден всегда (AC3). home резолвится бэкендом по ratified
// цепочке (DEVAI_HOME > ~/.devai); фронт только показывает данные сессии.
// Бейдж «Ожидания» (REQ-GATE-08): число строк секции на том же снимке
// данных — поле snapshot'а, вычисленное платформенной композицией, UI
// ничего не пересчитывает и не кэширует между load.
import { onMounted, onUnmounted, ref } from "vue";
import { api, toUiError } from "./api";
import { fmtTime, pollMs } from "./settings";
import type { SessionInfo, UiError } from "./types";

const session = ref<SessionInfo | null>(null);
const error = ref<UiError | null>(null);
const waitBadge = ref(0);
let timer: number | undefined;

async function loadBadge() {
  try {
    const snap = await api.inboxView();
    waitBadge.value = snap.badge;
  } catch (e) {
    // Статус-бар owns its facts; the badge keeps its last published value.
    error.value = toUiError(e);
  }
}

onMounted(async () => {
  try {
    session.value = await api.sessionInfo();
  } catch (e) {
    error.value = toUiError(e);
  }
  await loadBadge();
  timer = window.setInterval(loadBadge, pollMs());
});
onUnmounted(() => {
  if (timer) window.clearInterval(timer);
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
        <RouterLink to="/inbox">
          Ожидание<template v-if="waitBadge > 0">&nbsp;·&nbsp;<b class="badge">{{ waitBadge }}</b></template>
        </RouterLink>
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
