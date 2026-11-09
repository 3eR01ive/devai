<script setup lang="ts">
// Каркас окна (REQ-UI-01): навигация + статус-бар с фактом открытого
// home — путь виден всегда (AC3). home резолвится бэкендом по ratified
// цепочке (DEVAI_HOME > ~/.devai) и регистрируется в реестре домов
// (REQ-HOME-02); реестр живёт в контрольном доме (OQ-HOME-1). Фронт только
// показывает данные сессии, включая честный факт деградированного открытия
// (REQ-HOME-07).
// Бейдж «Ожидания» (REQ-GATE-08): число строк секции на том же снимке
// данных — поле snapshot'а, вычисленное платформенной композицией, UI
// ничего не пересчитывает и не кэширует между load. Опрос — ОДИН на всё
// приложение: шапка читает общий стор секции (inbox.ts),
// собственного таймера и второго вызова `inbox_view` здесь нет.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { toUiError } from "./api";
import { degradedBanner } from "./homes-view";
import { subscribeWait, waitError, waitSnapshot } from "./inbox";
import { sessionStore } from "./session";
import { fmtTime } from "./settings";
import type { SessionInfo, UiError } from "./types";

// Сессия — не снимок момента монтирования (R2 quality return): стор
// сессии один на окно, активация дома публикует в него свежий SessionInfo,
// и статус-бар/деградированный баннер перерисовываются без перезагрузки.
const session = ref<SessionInfo | null>(sessionStore.get());
const error = ref<UiError | null>(null);
const shownError = computed(() => error.value ?? waitError.value);
const waitBadge = computed(() => waitSnapshot.value?.badge ?? 0);
const degraded = computed(() => degradedBanner(session.value));
let unsubscribe: (() => void) | undefined;
const stopWatching = sessionStore.subscribe((s) => {
  session.value = s;
});

onMounted(async () => {
  try {
    await sessionStore.refresh();
  } catch (e) {
    error.value = toUiError(e);
  }
  unsubscribe = subscribeWait();
});
onUnmounted(() => {
  unsubscribe?.();
  stopWatching();
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
        <RouterLink to="/homes">Дома</RouterLink>
        <RouterLink to="/models">Связка модели</RouterLink>
        <RouterLink to="/lan">Телефон</RouterLink>
        <RouterLink to="/ops">Операции API</RouterLink>
      </nav>
      <div class="statusbar" v-if="session">
        <span class="st-home"><span class="st-label">активный home </span><b>{{ session.home }}</b></span>
        <span class="st-registry">
          <span class="st-label">реестр в </span><b>{{ session.control_home }}</b> · actor
          <b>{{ session.actor }}</b> · снимок {{ fmtTime(session.opened_at) }}
        </span>
      </div>
    </header>
    <main class="content">
      <div v-if="shownError" class="errorbox">
        <span class="code">{{ shownError.code }}</span>{{ shownError.message }}
      </div>
      <div v-if="degraded" class="errorbox">
        <span class="code">{{ session?.degraded_from?.code }}</span>{{ degraded }}
      </div>
      <RouterView />
    </main>
  </div>
</template>
