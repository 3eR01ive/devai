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
import { computed } from "vue";
import { useRoute } from "vue-router";
import { pairRefusal } from "../pairState";

const route = useRoute();
const reason = computed(() => {
  const value = route.query.reason;
  return typeof value === "string" ? value : "";
});
const connecting = computed(() => route.query.step === "connect");
const refusal = computed(() => (connecting.value ? null : pairRefusal(reason.value)));
const canReturn = computed(() => route.query.return === "1");
</script>

<template>
  <section class="panel" data-testid="pair-screen">
    <template v-if="connecting">
      <h2 style="margin: 0 0 8px">подключаюсь к дому…</h2>
      <p class="pair-hint muted" data-testid="pair-connect">
        Приглашение принято. Спрашиваю машину — если она в этой сети и приглашение
        ещё живо, её интерфейс откроется в этом же окне.
      </p>
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
