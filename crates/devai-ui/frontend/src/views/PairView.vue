<script setup lang="ts">
// Экран отказа в приглашении (task 01a1bf78, REQ-QRAPP-06).
//
// Сюда ведёт нативная половина приложения: `#/pair?reason=<код>` — либо на
// странице открытого дома (просроченное/испорченное НОВОЕ приглашение, дом при
// этом остаётся работать), либо на собранном интерфейсе приложения (машина
// замолчала: «доступ закрыт» или «не та сеть»). Слова берутся из единственной
// декларации ./pairState, своей фразы экран не придумывает.
import { computed } from "vue";
import { useRoute } from "vue-router";
import { pairRefusal } from "../pairState";

const route = useRoute();
const reason = computed(() => {
  const value = route.query.reason;
  return typeof value === "string" ? value : "";
});
const refusal = computed(() => pairRefusal(reason.value));
// Дом, который уже был открыт, живой только у двух причин: у остальных машина
// сама не отвечает, и «вернуться» некуда.
const canReturn = computed(
  () => reason.value === "invite_expired" || reason.value === "pair_bad_link",
);
</script>

<template>
  <section class="panel" data-testid="pair-screen">
    <h2 style="margin: 0 0 8px">
      {{ refusal?.title ?? "приглашение не принято" }}
    </h2>
    <p v-if="refusal" class="pair-hint" data-testid="pair-hint">{{ refusal.hint }}</p>
    <p v-else class="muted">
      Приглашение не принято. Отсканируйте код из окна машины ещё раз.
    </p>
    <p v-if="reason === 'invite_expired' || reason === 'pair_bad_link'" class="muted">
      Открытый дом не тронут: его сессия жива, данные на месте.
    </p>
    <RouterLink v-if="canReturn" to="/">Вернуться к открытому дому</RouterLink>
  </section>
</template>

<style scoped>
.pair-hint {
  max-width: 560px;
  line-height: 1.45;
}
</style>
