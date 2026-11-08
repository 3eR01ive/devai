<script setup lang="ts">
// Поле обоснования и обе кнопки подписи (REQ-SIGN-04). Кнопка доступна ровно
// по may_approve/may_reject из ответа; при отказе кнопки нет, а причина
// названа словами. Срабатывание — только удержанием на самой кнопке: клика
// и модального подтверждения нет; отпускание раньше порога отменяет.
import { computed } from "vue";
import { HOLD_MS, choicesValid, rationaleValid, type OqChoice } from "../../signature_view";
import { useHoldToConfirm, type HoldKind } from "../signature/useHoldToConfirm";

const props = defineProps<{
  mayApprove: boolean;
  mayReject: boolean;
  approvalDenied: string | null;
  rejectDenied: string | null;
  rationale: string;
  choices: OqChoice[];
  busy: boolean;
}>();

const emit = defineEmits<{
  (e: "update:rationale", text: string): void;
  (e: "apply", kind: HoldKind, rationale: string): void;
}>();

const rationaleOk = computed(() => rationaleValid(props.rationale));
const choicesOk = computed(() => choicesValid(props.choices));
const canSign = computed(() => rationaleOk.value && choicesOk.value && !props.busy);

const { holding, progress, start, release } = useHoldToConfirm(
  () => canSign.value,
  (kind) => emit("apply", kind, props.rationale),
);
</script>

<template>
  <div class="actions-wrap">
    <!-- Обоснование: видимое правимое поле, то, что уходит в решение -->
    <label class="rationale">
      <span>Обоснование подписи (обязательно — уходит в решение как rationale)</span>
      <textarea
        :value="props.rationale"
        rows="4"
        placeholder="почему подписываю / почему отклоняю"
        @input="emit('update:rationale', ($event.target as HTMLTextAreaElement).value)"
      />
    </label>
    <p v-if="!rationaleOk" class="muted small hint">
      без непустого обоснования подпись недоступна.
    </p>
    <p v-else-if="!choicesOk" class="notice small hint">
      у каждого «своего ответа» на вопрос должен быть непустой текст.
    </p>

    <div class="rowflex actions">
      <button
        v-if="props.mayApprove"
        class="hold approve"
        :class="{ holding: holding === 'approve' }"
        :disabled="!canSign"
        @pointerdown.prevent="start('approve', $event)"
        @pointerup="release"
        @pointercancel="release"
        @pointerleave="release"
        @contextmenu.prevent
      >
        <span class="fill" :style="{ width: `${holding === 'approve' ? progress * 100 : 0}%` }" />
        <span class="label">
          {{ props.busy ? "подписываю…" : `Утвердить — удерживайте ${HOLD_MS} мс` }}
        </span>
      </button>
      <span v-else class="muted">
        «Утвердить» недоступно: {{ props.approvalDenied ?? "may_approve=false" }}
      </span>

      <button
        v-if="props.mayReject"
        class="hold reject"
        :class="{ holding: holding === 'reject' }"
        :disabled="!canSign"
        @pointerdown.prevent="start('reject', $event)"
        @pointerup="release"
        @pointercancel="release"
        @pointerleave="release"
        @contextmenu.prevent
      >
        <span class="fill" :style="{ width: `${holding === 'reject' ? progress * 100 : 0}%` }" />
        <span class="label">Отклонить — удерживайте {{ HOLD_MS }} мс</span>
      </button>
      <span v-else class="muted">
        «Отклонить» недоступно: {{ props.rejectDenied ?? "may_reject=false" }}
      </span>
    </div>
    <p v-if="props.mayReject" class="notice small hint">
      Отклонение переведёт задачу <b>APPROVAL → ANALYSIS</b>; кнопка срабатывает
      только удержанием, одиночный клик ничего не делает.
    </p>
  </div>
</template>

<style scoped>
.rationale {
  display: block;
  margin-top: 12px;
}
.rationale textarea {
  width: 100%;
  margin-top: 4px;
}
.actions {
  margin-top: 10px;
  gap: 10px;
  align-items: center;
}
.hold {
  position: relative;
  overflow: hidden;
  min-width: 240px;
}
.hold .fill {
  position: absolute;
  inset: 0;
  background: rgba(243, 106, 22, 0.25);
  pointer-events: none;
}
.hold .label {
  position: relative;
}
.hint {
  margin: 4px 0 0;
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
