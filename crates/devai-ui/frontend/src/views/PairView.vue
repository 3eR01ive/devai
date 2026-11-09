<script setup lang="ts">
// Экран приглашения (task 01a1bf78, REQ-QRAPP-02/06).
//
// Сюда ведёт нативная половина приложения:
//   * `#/pair?step=connect` — взятое приглашение, машина ещё не ответила:
//     человек видит, что скан принят, а не чужой домашний экран;
//   * `#/pair?reason=<код>` — отказ; слова из единственной декларации
//     ./pairState, своей фразы экран не придумывает.
//
// «Вернуться» показывается по признаку `return=1`, который ставит Rust, а не по
// списку причин: решение «есть ли куда возвращаться» принимает тот, кто знает
// состояние домов (качество R6: без доменных литералов в шаблоне).
//
// У экрана «подключаюсь…» есть выход (quality return R1): кнопка «продолжить»
// повторяет обмен, а по таймауту экран честно говорит, что машина не ответила,
// вместо вечного ожидания. Сам обмен по-прежнему проводит корень окна — здесь
// только просьба о нём, поэтому экран не знает ни про Tauri, ни про сеть.
import { computed, onUnmounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { PAIR_CONNECT_TIMEOUT_MS, requestPairingRun } from "../pairingWatch";
import { pairRefusal } from "../pairState";

const route = useRoute();
const reason = computed(() => {
  const value = route.query.reason;
  return typeof value === "string" ? value : "";
});
const connecting = computed(() => route.query.step === "connect");
const refusal = computed(() => (connecting.value ? null : pairRefusal(reason.value)));
const canReturn = computed(() => route.query.return === "1");
const stalled = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined;

// Ожидание принадлежит КОНКРЕТНОМУ скану: отпечаток `scan=` есть в адресе
// каждого приглашения (`lan::pair::connect_url`), поэтому повторный скан с
// того же экрана — новый ключ и честно новый отсчёт, а не унаследованный
// «машина не ответила» от прошлой попытки.
const runKey = computed(() =>
  connecting.value ? `connect:${String(route.query.scan ?? "")}` : "",
);

function stopTimer() {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
}

// Ушёл на отказ или вернулся к дому — таймер замолкает; пришёл новый скан —
// отсчитывается заново.
watch(
  runKey,
  (key) => {
    stopTimer();
    stalled.value = false;
    if (key) timer = setTimeout(() => (stalled.value = true), PAIR_CONNECT_TIMEOUT_MS);
  },
  { immediate: true },
);
onUnmounted(stopTimer);

function continueRun() {
  stopTimer();
  stalled.value = false;
  timer = setTimeout(() => (stalled.value = true), PAIR_CONNECT_TIMEOUT_MS);
  requestPairingRun();
}
</script>

<template>
  <section class="panel" data-testid="pair-screen">
    <template v-if="connecting">
      <h2 style="margin: 0 0 8px">подключаюсь к дому…</h2>
      <p class="pair-hint muted" data-testid="pair-connect">
        Приглашение принято. Спрашиваю машину — если она в этой сети и приглашение
        ещё живо, её интерфейс откроется в этом же окне.
      </p>
      <p v-if="stalled" class="pair-hint" data-testid="pair-stalled">
        Машина пока не ответила. Проверьте, что телефон в сети этой машины, и
        попробуйте ещё раз — приглашение от этого не сгорает.
      </p>
      <button data-testid="pair-continue" @click="continueRun">продолжить</button>
    </template>
    <template v-else>
      <h2 style="margin: 0 0 8px">
        {{ refusal?.title ?? "приглашение не принято" }}
      </h2>
      <p v-if="refusal" class="pair-hint" data-testid="pair-hint">{{ refusal.hint }}</p>
      <p v-else class="muted">
        Приглашение не принято. Отсканируйте код из окна машины ещё раз.
      </p>
      <p v-if="canReturn" class="muted">
        Открытый дом не тронут: его сессия жива, данные на месте.
      </p>
      <RouterLink v-if="canReturn" to="/">Вернуться к открытому дому</RouterLink>
    </template>
  </section>
</template>

<style scoped>
.pair-hint {
  max-width: 560px;
  line-height: 1.45;
}
</style>
