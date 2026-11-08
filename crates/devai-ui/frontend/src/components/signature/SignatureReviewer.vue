<script setup lang="ts">
// Блок ревьювера (REQ-SIGN-11): вердикт сохранённым токеном и абзац отчёта
// (или summary с честной меткой), рядом seq и статус отчёта. Показывается
// ВСЕГДА — и при spec=null (REQ-SIGN-01 AC4); при отсутствии отчёта говорит
// словами `review_note`, а не пустым блоком и не красной плашкой.
import { computed } from "vue";
import type { ReviewView } from "../../types";
import { ARTIFACT_STATUS_LABELS, labelFor } from "../../signature_labels";
import { reviewParagraph, reviewVerdict } from "../../signature_view";

const props = defineProps<{
  review: ReviewView | null;
  note: string | null;
}>();

const text = computed(() => (props.review ? reviewParagraph(props.review) : null));
const verdict = computed(() => (props.review ? reviewVerdict(props.review) : null));
const status = computed(() =>
  props.review ? labelFor(ARTIFACT_STATUS_LABELS, props.review.artifact.status).text : null,
);
</script>

<template>
  <div class="reviewer">
    <h4 class="subhead">
      Отчёт ревьювера
      <template v-if="review">
        · версия {{ review.artifact.seq }}
        <span class="muted">· {{ status }}</span>
        <span v-if="!review.current" class="muted">(не CURRENT)</span>
      </template>
    </h4>
    <template v-if="review">
      <div class="rowflex">
        <span class="mono">вердикт: {{ verdict?.token }}</span>
        <span class="muted">{{ verdict?.word }}</span>
      </div>
      <p class="muted small source">
        {{ text?.sourceNote }}
        <template v-if="text?.note">· {{ text.note }}</template>
      </p>
      <p class="paragraph">{{ text?.text }}</p>
    </template>
    <p v-else class="muted">{{ note ?? "отчёта ревьювера нет" }}</p>
  </div>
</template>

<style scoped>
.subhead {
  margin: 14px 0 6px;
  font-size: 13px;
  color: var(--slate);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.source {
  margin: 2px 0 4px;
}
.paragraph {
  margin: 0 0 10px;
  white-space: pre-wrap;
}
.small {
  font-size: 12px;
}
</style>
