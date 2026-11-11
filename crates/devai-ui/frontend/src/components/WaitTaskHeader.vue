<script setup lang="ts">
// Общая шапка карточки ожидания (R2, mt/dry): заголовок-ссылка на drill
// /task/:id, чипы состояния и return_state, метка давности. Один markup
// для строк «платформа стоит» и «остановлено оператором» — дрейф двух
// почти одинаковых копий исключён. Дополнительные фрагменты (demand,
// CLI-подсказка) приходят слотом; клик по шапке обрабатывает родитель.
import { fmtTime, shortId } from "../settings";
import StateChip from "./StateChip.vue";
import type { Task } from "../types";

defineProps<{ task: Task; timeLabel: string }>();
</script>

<template>
  <h3>
    <RouterLink class="title-link" :to="`/task/${task.id}`" @click.stop>{{ task.display }}</RouterLink>
    <span class="mono muted">· {{ shortId(task.id) }}</span>
  </h3>
  <p
    v-if="task.display !== task.title"
    class="muted mono"
    style="margin: 0 0 4px"
  >формальный заголовок: {{ task.title }}</p>
  <div class="rowflex" style="margin-bottom: 6px">
    <StateChip :token="task.state" />
    <StateChip v-if="task.return_state" :token="task.return_state" />
    <slot />
    <span class="muted mono">{{ timeLabel }} {{ fmtTime(task.updated_at) }}</span>
  </div>
</template>
