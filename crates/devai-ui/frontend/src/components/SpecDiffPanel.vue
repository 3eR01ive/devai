<script setup lang="ts">
// Панель диффа спеки к прошлой версии (REQ-SIGN-13): человеческое построчное
// сопоставление РАЗОБРАННЫХ полей (абзац и требования), +/− и неизменённый
// контекст. Сырой JSON, sha и размеры не печатаются; дифф не сохраняется и
// артефактом не публикуется — это представление.
import type { PreviousView } from "../types";

defineProps<{ previous: PreviousView }>();
</script>

<template>
  <div id="spec-diff" class="panel">
    <h3>
      Дифф спеки · предыдущая версия {{ previous.artifact.seq }}
      <span class="muted mono" style="font-weight: 400">{{ previous.artifact.name }}</span>
    </h3>
    <div class="body">
      <p v-if="previous.diff.note" class="notice">{{ previous.diff.note }}</p>
      <template v-else>
        <h4 class="subhead">Абзац</h4>
        <p v-if="!previous.diff.paragraph.length" class="muted" style="margin: 0">
          абзаца нет ни в одной версии
        </p>
        <div v-for="(l, i) in previous.diff.paragraph" :key="`p-${i}`" class="dline" :class="l.mark === '+' ? 'add' : l.mark === '-' ? 'del' : 'ctx'">
          <span class="mark mono">{{ l.mark }}</span>
          <span class="text">{{ l.text }}</span>
        </div>

        <h4 class="subhead">Требования</h4>
        <p v-if="!previous.diff.requirements.length" class="muted" style="margin: 0">
          требований нет ни в одной версии
        </p>
        <div v-for="(l, i) in previous.diff.requirements" :key="`r-${i}`" class="dline" :class="l.mark === '+' ? 'add' : l.mark === '-' ? 'del' : 'ctx'">
          <span class="mark mono">{{ l.mark }}</span>
          <span class="label mono muted">{{ l.label }}</span>
          <span class="text">{{ l.text }}</span>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.subhead {
  margin: 12px 0 6px;
  font-size: 13px;
  color: var(--slate);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.dline {
  display: flex;
  gap: 8px;
  white-space: pre-wrap;
  padding: 1px 4px;
}
.dline .mark {
  width: 12px;
}
.dline.add {
  background: rgba(111, 143, 106, 0.18);
}
.dline.del {
  background: rgba(176, 106, 106, 0.18);
}
.dline.ctx {
  color: var(--slate);
}
.label {
  font-size: 11px;
  min-width: 90px;
}
.notice {
  border-left: 3px solid var(--orange);
  padding: 4px 8px;
  color: var(--orange);
}
</style>
