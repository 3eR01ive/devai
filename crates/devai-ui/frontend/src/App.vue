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
import { computed, onErrorCaptured, onMounted, onUnmounted, ref } from "vue";
import { pairingShell, toUiError } from "./api";
import { degradedBanner } from "./homes-view";
import { subscribeWait, waitError, waitSnapshot } from "./inbox";
import { activeSnapshot, subscribeActive } from "./active";
import { pairFailure, pairingNotice, readNotice, subscribePairing } from "./pairing";
import { windowBridge } from "./pairingNative";
import type { PairTask } from "./pairingNative";
import { pairingRunner, setPairingRunner, watchScans } from "./pairingWatch";
import { pairNotice } from "./pairState";
import { sessionStore } from "./session";
import { fmtTime } from "./settings";
import type { SessionInfo, UiError } from "./types";

// Сессия — не снимок момента монтирования (R2 quality return): стор
// сессии один на окно, активация дома публикует в него свежий SessionInfo,
// и статус-бар/деградированный баннер перерисовываются без перезагрузки.
const session = ref<SessionInfo | null>(sessionStore.get());
const error = ref<UiError | null>(null);
const shownError = computed(() => error.value ?? waitError.value);
// Цифры на вкладке задач: сколько идёт прямо сейчас и сколько ждёт человека.
const runningCount = computed(() => activeSnapshot.value?.rows?.length ?? 0);
const waitingCount = computed(() => waitSnapshot.value?.waiting?.length ?? 0);
const degraded = computed(() => degradedBanner(session.value));
// Ошибка отрисовки экрана не должна оставлять пустое окно: белый (тёмный) экран
// без единого слова — худший вид поломки, человек не знает даже, что случилось.
const renderError = ref<string | null>(null);
onErrorCaptured((e: unknown) => {
  // ВРЕМЕННО (отладка): показываем стек, чтобы поймать источник поломки.
  renderError.value = e instanceof Error ? e.message : String((e as { message?: string })?.message ?? e);
  return false;
});
function clearRenderError() {
  renderError.value = null;
}
// Заметка о том, что доступ есть, но что-то не сохранилось (адрес дома):
// код приехал в адресе открытой страницы, слова — из единственной декларации.
const notice = computed(() => pairNotice(pairingNotice.value));
let unsubscribe: (() => void) | undefined;
let unsubscribeActive: (() => void) | undefined;
let stopPairing: (() => void) | undefined;
let stopScans: (() => void) | undefined;
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
  unsubscribeActive = subscribeActive();
  // Отзыв доступа замечает сама страница, которую отдала машина — и в
  // браузере, и в окне приложения это один и тот же опрос `/pair`
  // (REQ-QRAPP-05; сеть в Rust запрещена гейтом `no-network`).
  stopPairing = subscribePairing();
  pairingNotice.value = readNotice(window.location.search);
  // Приглашение проводит СВОЯ страница приложения: у неё есть мост в Rust.
  // Сбой самого моста — тоже видимый факт, а не тихо пропущенный запуск.
  // Один и тот же проход запускают две вещи: первый экран после загрузки и
  // смена адреса на экран принятого скана — тёплый скан не перезагружает
  // документ, поэтому `onMounted` здесь уже не сработает (quality return R1).
  if (pairingShell) {
    const run = pairingRunner(
      windowBridge(),
      (done: PairTask) => {
        // Нечего открывать (старый экран приглашения без ссылки) — возвращаемся
        // к собственному дому окна, а не оставляем человека на служебной странице.
        if (done.kind === "idle" && window.location.hash.startsWith("#/pair")) {
          window.location.hash = "#/";
        }
      },
      (e) => {
        error.value = toUiError(e);
      },
    );
    stopScans = watchScans(window, run);
    setPairingRunner(run);
    run();
  }
});
// Смена адреса снимает прошлую ошибку: экран новый, ошибка старая.
window.addEventListener("hashchange", clearRenderError);
onUnmounted(() => {
  window.removeEventListener("hashchange", clearRenderError);
  unsubscribe?.();
  unsubscribeActive?.();
  stopPairing?.();
  stopScans?.();
  setPairingRunner(null);
  stopWatching();
});
</script>

<template>
  <div class="shell">
    <header class="topbar">
      <span class="brand">dev<em>ai</em>-ui</span>
      <nav class="modes">
        <RouterLink to="/projects">Проекты</RouterLink>
        <RouterLink to="/proto/tasks">
          Задачи<template v-if="runningCount > 0 || waitingCount > 0">
            &nbsp;·&nbsp;<b class="badge">{{ runningCount }} идёт</b>
            <template v-if="waitingCount > 0">&nbsp;·&nbsp;<b class="badge badge-human">{{ waitingCount }} ждёт</b></template>
          </template>
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
    <!-- `data-scroll-host` — единственный прокручиваемый контейнер окна:
         его находит память прокрутки при запуске окна (main.ts), чтобы
         каждый экран помнил своё место сам. -->
    <main class="content" data-scroll-host>
      <div v-if="shownError" class="errorbox">
        <span class="code">{{ shownError.code }}</span>{{ shownError.message }}
      </div>
      <div v-if="degraded" class="errorbox">
        <span class="code">{{ session?.degraded_from?.code }}</span>{{ degraded }}
      </div>
      <!-- Доступ отозван (или машина замолчала): экран честно говорит, что
           случилось, и не стирает уже показанные данные молча — они остаются,
           но помечены как последние (REQ-QRAPP-05 AC2, REQ-QRAPP-06 AC5). -->
      <div v-if="pairFailure" class="errorbox" data-testid="pair-refusal">
        <b>{{ pairFailure.title }}.</b> {{ pairFailure.hint }}
        <div class="muted">
          Данные ниже — последние полученные, пока доступ был открыт; обновляться
          они больше не будут.
        </div>
      </div>
      <!-- Доступ есть, но кое-что не сохранилось (адрес дома): код приехал в
           адресе страницы, поэтому предупреждение видно и после перехода
           (качество: R1 — проглоченная ошибка записи). -->
      <div v-if="notice" class="errorbox" data-testid="pair-notice">
        <span class="code">{{ pairingNotice }}</span>{{ notice.title }}. {{ notice.hint }}
      </div>
      <div v-if="renderError" class="render-error">
        <b>экран не отрисовался:</b> {{ renderError }}
        <button @click="$router.go(0)">перезагрузить</button>
      </div>
      <RouterView />
    </main>
  </div>
</template>
