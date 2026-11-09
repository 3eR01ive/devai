<script setup lang="ts">
// Действия гейта качества (REQ-QG-01..03/05/08): «Принять» и «Вернуть в
// доработку» — те же акты, что `devai task accept|return`. Доступность берётся
// из `gate` (публичное чтение платформы), окно добавляет только непустое
// обоснование. Срабатывание — удержанием; клика и модального подтверждения нет.
import { computed, ref } from "vue";
import {
  acceptPreview,
  acceptRationale,
  canAccept,
  canReturn,
  composeReturnRationale,
  editDraftRequirement,
  rationaleHint,
  removeDraftItem,
  returnPreview,
  siteText,
  type GateAct,
  type ReturnDraftItem,
} from "../../quality_gate_view";
import { HOLD_MS } from "../../signature_view";
import type { QualityGateView, UiError } from "../../types";
import { useHoldToConfirm } from "../signature/useHoldToConfirm";

const props = defineProps<{
  gate: QualityGateView;
  draft: ReturnDraftItem[];
  busy: boolean;
  error: UiError | null;
}>();

const emit = defineEmits<{
  (e: "update:draft", draft: ReturnDraftItem[]): void;
  (e: "apply", kind: GateAct, rationale: string): void;
}>();

const rationale = ref("");
const acceptOk = computed(() => canAccept(props.gate, rationale.value));
const returnOk = computed(() => canReturn(props.gate, rationale.value, props.draft));
const canStart = computed(
  () => !props.busy && ((acceptOk.value && props.gate.may_accept) || (returnOk.value && props.gate.may_return)),
);

const { holding, progress, start, release } = useHoldToConfirm(
  () => canStart.value,
  (kind) => {
    if (kind === "accept" && acceptOk.value) {
      emit("apply", "accept", acceptRationale(rationale.value));
    } else if (kind === "return" && returnOk.value) {
      emit("apply", "return", composeReturnRationale(rationale.value, props.draft));
    }
  },
);

function setRequirement(key: string, text: string) {
  emit("update:draft", editDraftRequirement(props.draft, key, text));
}
function remove(key: string) {
  emit("update:draft", removeDraftItem(props.draft, key));
}
function clearAll() {
  emit("update:draft", []);
}
</script>

<template>
  <div class="actions-wrap">
    <!-- Черновик возврата (REQ-QG-05): собранные из отчёта пункты, каждый
         правится; пустое требование в отправляемый текст не попадает. -->
    <div class="draft">
      <div class="rowflex draft-head">
        <b>Черновик возврата</b>
        <span class="muted">{{ props.draft.length }} пункт(ов)</span>
        <div class="spacer" />
        <button v-if="props.draft.length" class="clear" @click="clearAll">очистить черновик</button>
      </div>
      <p v-if="!props.draft.length" class="muted small">
        добавьте пункты из списка проверок — требование предзаполнится сохранённой
        сутью и останется правимым. Без черновика возврат уходит с одним обоснованием.
      </p>
      <div v-for="d in props.draft" :key="d.key" class="draft-item">
        <div class="rowflex">
          <span class="mono muted small">{{ siteText(d.site) }}</span>
          <div class="spacer" />
          <button class="remove" @click="remove(d.key)">убрать</button>
        </div>
        <textarea
          rows="2"
          :value="d.requirement"
          placeholder="требование к правке"
          @input="setRequirement(d.key, ($event.target as HTMLTextAreaElement).value)"
        />
      </div>
    </div>

    <label class="rationale">
      <span>Обоснование (обязательно — уходит в решение как rationale)</span>
      <textarea
        v-model="rationale"
        rows="4"
        placeholder="почему принимаю / почему возвращаю"
      />
    </label>

    <!-- Что произойдёт ДО действия (REQ-QG-03 AC1/AC2). -->
    <p class="preview small">{{ acceptPreview() }}</p>
    <p class="preview small" style="white-space: pre-wrap">{{ returnPreview(props.draft) }}</p>

    <!-- Отказ платформы — РЯДОМ С КНОПКАМИ (REQ-QG-03 AC4/AC5): код и текст
         как есть, введённое обоснование и черновик не сбрасываются. -->
    <div v-if="props.error" class="gate-error">
      <span class="code">{{ props.error.code }}</span> {{ props.error.message }}
    </div>

    <div class="rowflex actions">
      <div class="act">
        <button
          class="hold approve"
          :class="{ holding: holding === 'accept' }"
          :disabled="!props.gate.may_accept || !acceptOk || props.busy"
          @pointerdown.prevent="start('accept', $event)"
          @pointerup="release"
          @pointercancel="release"
          @pointerleave="release"
          @contextmenu.prevent
        >
          <span class="fill" :style="{ width: `${holding === 'accept' ? progress * 100 : 0}%` }" />
          <span class="label">
            {{ props.busy
              ? "выполняю…"
              : !props.gate.may_accept
                ? "недоступно — причина ниже"
                : !acceptOk
                  ? "сначала обоснование"
                  : `Принять — удерживайте ${HOLD_MS} мс` }}
          </span>
        </button>
        <p v-if="!props.gate.may_accept && props.gate.accept_denied" class="denied small">
          принять нельзя: {{ props.gate.accept_denied.text }}
        </p>
        <p v-else-if="rationaleHint('accept', rationale, props.draft)" class="hint small">
          {{ rationaleHint("accept", rationale, props.draft) }}
        </p>
      </div>

      <div class="act">
        <button
          class="hold rework"
          :class="{ holding: holding === 'return' }"
          :disabled="!props.gate.may_return || !returnOk || props.busy"
          @pointerdown.prevent="start('return', $event)"
          @pointerup="release"
          @pointercancel="release"
          @pointerleave="release"
          @contextmenu.prevent
        >
          <span class="fill" :style="{ width: `${holding === 'return' ? progress * 100 : 0}%` }" />
          <span class="label">
            {{ props.busy
              ? "выполняю…"
              : !props.gate.may_return
                ? "недоступно — причина ниже"
                : !returnOk
                  ? "сначала обоснование или пункты"
                  : `Вернуть в доработку — удерживайте ${HOLD_MS} мс` }}
          </span>
        </button>
        <p v-if="!props.gate.may_return && props.gate.return_denied" class="denied small">
          вернуть нельзя: {{ props.gate.return_denied.text }}
        </p>
        <p v-else-if="rationaleHint('return', rationale, props.draft)" class="hint small">
          {{ rationaleHint("return", rationale, props.draft) }}
        </p>
      </div>
    </div>
    <p class="muted small">
      кнопки срабатывают только удержанием; отпускание раньше порога ничего не делает.
    </p>
  </div>
</template>

<style scoped>
.actions-wrap {
  margin-top: 12px;
  border-top: 1px solid var(--panel-border);
  padding-top: 10px;
}
.gate-error {
  margin: 8px 0;
  padding: 6px 10px;
  border-left: 3px solid #d33;
  background: #fdecec;
  color: #7a1010;
  white-space: pre-wrap;
}
.gate-error .code {
  font-family: var(--mono, monospace);
  margin-right: 8px;
  font-weight: 600;
}
.draft {
  margin-bottom: 10px;
}
.draft-head {
  margin-bottom: 4px;
}
.draft-item {
  border: 1px solid var(--panel-border);
  border-radius: 4px;
  padding: 6px;
  margin-top: 6px;
}
.draft-item textarea {
  width: 100%;
  margin-top: 4px;
}
.rationale {
  display: block;
}
.rationale textarea {
  width: 100%;
  margin-top: 4px;
}
.preview {
  margin: 6px 0 0;
}
.actions {
  margin-top: 10px;
  gap: 14px;
  align-items: flex-start;
}
.act {
  flex: 1;
}
.hold {
  position: relative;
  overflow: hidden;
  width: 100%;
  min-width: 220px;
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
.hold:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  filter: grayscale(0.4);
}
.hint,
.denied {
  margin: 4px 0 0;
}
.denied {
  color: #7a1010;
}
.small {
  font-size: 12px;
}
.clear,
.remove {
  font-size: 12px;
}
</style>
