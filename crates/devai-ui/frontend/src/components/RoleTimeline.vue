<script setup lang="ts">
// Горизонтальный путь задачи для списка: узлы этапов, пройденное — сплошной
// линией, текущий — акцентом, будущее — пунктиром. Подписи короткие, чтобы
// строка списка оставалась строкой. Никаких своих таблиц «состояние →
// подпись»: слова и порядок приходят готовыми в graph.stages.
import { computed } from "vue";
import type { GraphStage } from "../types";

const props = defineProps<{ stages: GraphStage[]; state?: string }>();

const SHORT: Record<string, string> = {
  STATEMENT: "постановка",
  ANALYSIS: "анализ",
  REVIEW: "ревью",
  APPROVAL: "подпись",
  IMPLEMENTATION: "код",
  QA: "приёмка",
};

const steps = computed(() =>
  props.stages.map((s) => ({
    short: SHORT[s.state] ?? s.label,
    status: s.status,
    gate: s.gate,
    word: s.status_word,
  })),
);

const passed = computed(() => steps.value.filter((s) => s.status === "passed").length);
</script>

<template>
  <div v-if="!steps.length" class="tl muted">путь недоступен</div>
  <div v-else class="tl" :title="`пройдено ${passed} из ${steps.length}`">
    <div v-for="(s, i) in steps" :key="i" class="step" :class="[s.status, { gate: s.gate }]">
      <div v-if="i" class="rail" />
      <div class="node" />
      <div class="cap">{{ s.short }}</div>
    </div>
  </div>
</template>

<style scoped>
.tl{display:flex;align-items:flex-start;padding-top:2px;min-width:280px}
.step{flex:1;display:flex;flex-direction:column;align-items:center;position:relative}
.rail{position:absolute;top:6px;left:50%;width:100%;height:2px;background:rgba(113,128,141,.25)}
.step.passed .rail{background:#3d7a5c}
.node{position:relative;z-index:2;width:12px;height:12px;border-radius:50%;
  border:2px solid var(--panel-border);background:rgba(17,24,32,.9)}
.step.passed .node{background:#4f9d76;border-color:#4f9d76}
.step.current .node{width:16px;height:16px;margin-top:-2px;background:var(--orange);
  border-color:#ffb07a;box-shadow:0 0 0 4px rgba(243,106,22,.16)}
.step.gate .node{border-radius:2px;transform:rotate(45deg)}
.cap{margin-top:5px;font-size:9.5px;color:var(--slate);white-space:nowrap}
.step.current .cap{color:var(--orange);font-weight:600}
.step.passed .cap{color:#8fa3b3}
.muted{color:var(--slate);font-size:11px}

@media (max-width: 767px) {
  .cap, .muted { font-size: 12px; }
}
</style>
