<script setup lang="ts">
// Абзац, статус и требования спеки (REQ-SIGN-02): печатается сохранённый
// текст и id как есть; id вторичен (mono muted); критерии раскрываются.
// Компонент только раскладывает готовые значения `signature_view.ts`.
import { computed } from "vue";
import type { SpecView } from "../../types";
import { ARTIFACT_STATUS_LABELS, labelFor } from "../../signature_labels";
import { criteriaSummary, specParagraph } from "../../signature_view";

const props = defineProps<{ spec: SpecView }>();

const paragraph = computed(() => specParagraph(props.spec));
</script>

<template>
  <div class="spec-body">
    <p v-if="!spec.current" class="notice">
      показана последняя спека, но она <b>не CURRENT</b> (версия
      {{ spec.artifact.seq }}) — утвердить нельзя, отклонить можно.
    </p>

    <!-- Абзац целиком, до формальной структуры -->
    <p class="muted source">
      {{ paragraph.sourceNote }}
      <template v-if="paragraph.note">· {{ paragraph.note }}</template>
    </p>
    <p class="paragraph">{{ paragraph.text }}</p>

    <div class="rowflex meta">
      <span class="muted">статус спеки:</span>
      <span>{{ labelFor(ARTIFACT_STATUS_LABELS, spec.artifact.status).text }}</span>
    </div>

    <!-- Требования: id вторично и приглушённо -->
    <h4 class="subhead">Требования ({{ spec.requirements.length }})</h4>
    <p v-if="spec.requirements_note" class="muted note">{{ spec.requirements_note }}</p>
    <div v-for="(r, i) in spec.requirements" :key="r.id || i" class="req">
      <div class="rowflex">
        <span class="req-id mono">{{ r.id || "—" }}</span>
        <span class="req-statement">{{ r.statement }}</span>
      </div>
      <details>
        <summary class="muted small">
          критерии и ограничения: {{ criteriaSummary(r.acceptance_criteria.length, r.constraints.length) }}
        </summary>
        <ul class="ac">
          <li v-for="(ac, j) in r.acceptance_criteria" :key="`ac-${j}`">{{ ac }}</li>
          <li v-for="(c, j) in r.constraints" :key="`c-${j}`" class="muted">
            ограничение: {{ c }}
          </li>
        </ul>
      </details>
    </div>
    <p v-if="!spec.requirements.length && !spec.requirements_note" class="muted empty">
      требований в спеке нет
    </p>
  </div>
</template>

<style scoped>
.paragraph {
  margin: 0 0 10px;
  white-space: pre-wrap;
}
.source {
  margin: 0 0 4px;
}
.note {
  margin: 0 0 6px;
}
.empty {
  margin: 0;
}
.meta {
  margin-bottom: 4px;
}
.subhead {
  margin: 14px 0 6px;
  font-size: 13px;
  color: var(--slate);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.req {
  margin-bottom: 8px;
  padding-left: 10px;
  border-left: 2px solid var(--panel-border);
}
.req-id {
  color: var(--slate);
  font-size: 11px;
  margin-right: 8px;
}
.req-statement {
  flex: 1;
}
.ac {
  margin: 4px 0 0;
  padding-left: 18px;
}
.notice {
  border-left: 3px solid var(--orange);
  padding: 4px 8px;
  color: var(--orange);
}
.small {
  font-size: 12px;
}
</style>
