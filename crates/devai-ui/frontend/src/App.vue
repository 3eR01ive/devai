<script setup lang="ts">
// Каркас окна (REQ-UI-01): навигация + статус-бар с фактом открытого
// home — путь виден всегда (AC3). home резолвится бэкендом по ratified
// цепочке (DEVAI_HOME > ~/.devai); фронт только показывает данные сессии.
// Бейдж «Ожидания» (REQ-GATE-08): число строк секции на том же снимке
// данных — поле snapshot'а, вычисленное платформенной композицией, UI
// ничего не пересчитывает и не кэширует между load. Опрос — ОДИН на всё
// приложение: шапка читает общий стор секции (inbox.ts),
// собственного таймера и второго вызова `inbox_view` здесь нет.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { api, toUiError } from "./api";
import { subscribeWait, waitError, waitSnapshot } from "./inbox";
import { fmtTime } from "./settings";
import type { SessionInfo, UiError } from "./types";

const session = ref<SessionInfo | null>(null);
const error = ref<UiError | null>(null);
const shownError = computed(() => error.value ?? waitError.value);
const waitBadge = computed(() => waitSnapshot.value?.badge ?? 0);
let unsubscribe: (() => void) | undefined;

onMounted(async () => {
  try {
    session.value = await api.sessionInfo();
  } catch (e) {
    error.value = toUiError(e);
  }
  unsubscribe = subscribeWait();
});
onUnmounted(() => unsubscribe?.());
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
      <div v-if="shownError" class="errorbox">
        <span class="code">{{ shownError.code }}</span>{{ shownError.message }}
      </div>
      <RouterView />
    </main>
  </div>
</template>
