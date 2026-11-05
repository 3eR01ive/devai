<script setup lang="ts">
// Чип состояния/статуса: токен печатается всегда (REQ-UI-13 AC2 — статусы
// различимы не только цветом), цвет — группа из палитры лого.
import { computed } from "vue";

const props = defineProps<{ token: string | null | undefined }>();

const group = computed(() => {
  const t = props.token ?? "";
  if (t === "WAITING_FOR_INPUT") return "attention"; // оранжевый: линия ждёт оператора
  if (["CURRENT", "RUNNING", "OPEN", "alive"].includes(t)) return "live";
  if (["STALE", "SUPERSEDED", "ANSWERED", "CANCELLED", "unknown"].includes(t)) return "muted";
  if (["ACCEPTED", "COMPLETED"].includes(t)) return "done";
  if (["FAILED", "BLOCKED", "PAUSED", "dead"].includes(t)) return "held";
  return "idle";
});
</script>

<template>
  <span v-if="token" class="chip" :class="group">{{ token }}</span>
</template>

<style scoped>
.chip {
  display: inline-block;
  padding: 1px 8px;
  border-radius: 4px;
  font-family: ui-monospace, "JetBrains Mono", monospace;
  font-size: 12px;
  letter-spacing: 0.02em;
  border: 1px solid var(--slate);
  color: var(--cream);
  background: rgba(113, 128, 141, 0.15);
  white-space: nowrap;
}
.chip.attention {
  border-color: var(--orange);
  color: var(--orange);
  background: rgba(243, 106, 22, 0.12);
  font-weight: 700;
}
.chip.live {
  border-color: var(--orange);
  color: var(--cream);
}
.chip.done {
  border-color: #6f8f6a;
  color: #bcd4b4;
}
.chip.held {
  border-color: #b08a4f;
  color: #e8caa0;
}
.chip.muted {
  border-color: var(--slate);
  color: var(--slate);
}
.chip.idle {
  border-color: var(--slate);
  color: var(--cream);
}
</style>
