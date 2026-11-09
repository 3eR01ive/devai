<script setup lang="ts">
// Панель диффа спеки к прошлой версии (REQ-SIGN-13): человеческое построчное
// сопоставление РАЗОБРАННЫХ полей (абзац и требования), +/− и неизменённый
// контекст. Сырой JSON, sha и размеры не печатаются; дифф не сохраняется и
// артефактом не публикуется — это представление.
//
// Rework R1: у версии до протокола своего абзаца НЕТ. Подставленное в карточке
// бизнес-описание — не её текст, поэтому сторона называется своим состоянием
// (словарь секции), а не показывается пустым абзацем.
import type { PreviousView } from "../types";
import { DIFF_PARAGRAPH_STATE_LABELS, labelFor } from "../signature_labels";

defineProps<{ previous: PreviousView }>();

function stateWord(token: string): string {
  return labelFor(DIFF_PARAGRAPH_STATE_LABELS, token).text;
}
</script>

<template>
  <div id="spec-diff" class="panel">
    <h3>
      Дифф спеки · предыдущая версия {{ previous.artifact.seq }}
      <span class="muted mono" style="font-weight: 400">{{ previous.artifact.name }}</span>
    </h3>
    <div class="body">
      <p v-if="previous.diff.refusal" class="notice">{{ previous.diff.note }}</p>
      <template v-else>
        <h4 class="subhead">Абзац</h4>
        <p
          v-if="
            previous.diff.paragraph_previous !== 'stored' ||
            previous.diff.paragraph_current !== 'stored'
          "
          class="side-state muted"
        >
          прошлая версия — {{ stateWord(previous.diff.paragraph_previous) }},
          текущая — {{ stateWord(previous.diff.paragraph_current) }}
        </p>
        <p v-if="previous.diff.paragraph_note" class="notice">
          {{ previous.diff.paragraph_note }}
        </p>
        <p v-if="previous.diff.note" class="notice">{{ previous.diff.note }}</p>
        <p
          v-if="!previous.diff.paragraph.length && !previous.diff.paragraph_note"
          class="muted"
          style="margin: 0"
        >
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
.side-state {
  margin: 0 0 4px;
  font-size: 12px;
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
/* Телефон и планшет: колонка метки в диффе не мельче 12px (REQ-RESP-08 AC3). */
@media (max-width: 1199px) {
  .label {
    font-size: 12px;
    min-width: 0;
  }
}
.notice {
  border-left: 3px solid var(--orange);
  padding: 4px 8px;
  color: var(--orange);
}
</style>
