<script setup lang="ts">
// Секция «Спека на утверждение» (REQ-SIGN-01..04) — КОРЕНЬ СБОРКИ (rework
// R4): шапка с версией и ссылкой на дифф, затем блоки — абзац/требования
// (SignatureSpecBody), ратификация вопросов (SignatureRatification), отчёт
// ревьювера (SignatureReviewer, виден всегда) и подпись (SignatureActions, а
// машинерия удержания — в useHoldToConfirm). Ни один блок здесь не реализован
// заново: правила — в `signature_view.ts`, слова — в данных секции.
//
// Права подписи — только may_approve/may_reject из ответа task.signature; UI
// их не пересчитывает. Всё, что уйдёт в rationale, оператор видит в поле.
import { ref, watch } from "vue";
import type { SignatureView } from "../types";
import { denialText, mergeOqLines, type OqChoice } from "../signature_view";
import { SIGNATURE_KIND_LABELS, labelFor } from "../signature_labels";
import SignatureSpecBody from "./signature/SignatureSpecBody.vue";
import SignatureRatification from "./signature/SignatureRatification.vue";
import SignatureReviewer from "./signature/SignatureReviewer.vue";
import SignatureActions from "./signature/SignatureActions.vue";

const props = defineProps<{
  /** Ошибка подписи — показывается РЯДОМ С КНОПКАМИ (R: оператор смотрел на кнопки
   *  внизу и не видел красную строку вверху карточки — «нажимаю, ничего не происходит»). */
  error?: { code: string; message: string } | null;
  signature: SignatureView;
  busy: boolean;
}>();

const emit = defineEmits<{
  (e: "apply", kind: "approve" | "reject", rationale: string): void;
  (e: "open-diff"): void;
}>();

const rationale = ref("");
const choices = ref<OqChoice[]>([]);

// Вопросы документа приходят строками (контракт @4); «принять как есть» —
// положение по умолчанию (ASM-6), и R6-приёмка требует, чтобы ВЫБРАННОЕ
// значение попадало в rationale подписи: умолчания пишутся в поле сразу,
// а не только после первой правки выбора.
watch(
  () => props.signature.spec?.open_questions,
  (questions) => {
    choices.value = (questions ?? []).map(() => ({ accept: true, answer: "" }));
    rationale.value = mergeOqLines("", choices.value);
  },
  { immediate: true },
);

function onChoice(index: number, accept: boolean) {
  choices.value[index] = { ...choices.value[index], accept };
  rationale.value = mergeOqLines(rationale.value, choices.value);
}
function onAnswer(index: number, answer: string) {
  choices.value[index] = { ...choices.value[index], answer };
  rationale.value = mergeOqLines(rationale.value, choices.value);
}
</script>

<template>
  <div class="panel signature">
    <h3>
      Спека на утверждение
      <template v-if="props.signature.spec">
        · версия {{ props.signature.spec.artifact.seq }}
      </template>
      <span class="spacer" />
      <a
        v-if="props.signature.previous"
        href="#spec-diff"
        class="muted diff-link"
        @click.prevent="emit('open-diff')"
      >дифф к прошлой версии</a>
      <span v-else class="muted">{{ props.signature.previous_note ?? "прошлой версии нет" }}</span>
    </h3>
    <div class="body">
      <!-- spec=null: утверждать нечего, но отклонение может быть доступно -->
      <p v-if="!props.signature.spec" class="muted no-spec">
        У задачи не опубликовано ни одного спецификационного артефакта —
        <b>утверждать нечего</b>.
      </p>
      <template v-else>
        <SignatureSpecBody :spec="props.signature.spec" />
        <SignatureRatification
          v-if="props.signature.spec.open_questions.length"
          :questions="props.signature.spec.open_questions"
          :choices="choices"
          @choice="onChoice"
          @answer="onAnswer"
        />
      </template>

      <SignatureReviewer :review="props.signature.review" :note="props.signature.review_note" />

      <div v-if="props.error" class="sig-error">
        <span class="code">{{ props.error.code }}</span> {{ props.error.message }}
      </div>
      <SignatureActions
        :may-approve="props.signature.may_approve"
        :may-reject="props.signature.may_reject"
        :approval-denied="denialText(props.signature.may_approve_denied)"
        :reject-denied="denialText(props.signature.may_reject_denied)"
        :rationale="rationale"
        :choices="choices"
        :busy="props.busy"
        @update:rationale="rationale = $event"
        @apply="(kind, text) => emit('apply', kind, text)"
      />

      <p v-if="props.signature.signature" class="muted small signed">
        уже подписана: {{ labelFor(SIGNATURE_KIND_LABELS, props.signature.signature.kind).text }}
        — {{ props.signature.signature.actor }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.sig-error {
  margin: 8px 0;
  padding: 6px 10px;
  border-left: 3px solid #d33;
  background: #fdecec;
  color: #7a1010;
  white-space: pre-wrap;
}
.sig-error .code {
  font-family: var(--mono, monospace);
  margin-right: 8px;
  font-weight: 600;
}
.no-spec {
  margin: 0 0 10px;
}
.signed {
  margin: 10px 0 0;
  font-size: 12px;
}
.diff-link {
  font-weight: 400;
  font-size: 12px;
}
</style>
