<script setup lang="ts">
// Действия гейта качества (REQ-QG-01..03/05/08, REQ-SAY-01/05): «Принять» и
// «Вернуть в доработку» — те же акты, что `devai task accept|return`. Каждая
// кнопка подписана ПОСЛЕДСТВИЕМ по-человечески, без служебных состояний
// (ACCEPTED/REWORK) в первом слое. Доступность берётся из `gate` (публичное
// чтение платформы), окно добавляет только непустое обоснование. Черновик
// возврата приходит уже заполненным всеми видимыми замечаниями: человек
// снимает лишнее. Срабатывание — удержанием; клика и модального подтверждения
// нет.
import { computed, ref } from "vue";
import {
  ACCEPT_LABEL,
  RETURN_LABEL,
  acceptRationale,
  canAccept,
  canReturn,
  composeReturnRationale,
  editDraftRequirement,
  inDraft,
  rationaleHint,
  removeDraftItem,
  siteText,
  type GateAct,
  type ReturnDraftItem,
} from "../../quality_gate_view";
import { HOLD_MS } from "../../signature_view";
import type { FirstLayerRow, QualityGateView, UiError } from "../../types";
import { useHoldToConfirm } from "../signature/useHoldToConfirm";

const props = defineProps<{
  gate: QualityGateView;
  draft: ReturnDraftItem[];
  busy: boolean;
  error: UiError | null;
  /** Человеческая строка о состоянии черновика (есть ли что возвращать). */
  draftNote: string;
  /** «Не проверено»: добавляется в черновик одной кнопкой, по умолчанию — нет. */
  unchecked: FirstLayerRow[];
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
function addUnchecked(it: FirstLayerRow) {
  emit("update:draft", [
    ...props.draft,
    { key: it.id, item_id: it.id, summary: it.what, site: it.site, requirement: it.what },
  ]);
}
</script>

<template>
  <div class="actions-wrap">
    <!-- Решение идёт первым: две кнопки, каждая подписана последствием. -->
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
          <span class="label">{{ props.busy ? "выполняю…" : ACCEPT_LABEL }}</span>
        </button>
        <p class="muted small">удерживай — отпустишь раньше {{ HOLD_MS }} мс, ничего не произойдёт.</p>
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
          <span class="label">{{ props.busy ? "выполняю…" : RETURN_LABEL }}</span>
        </button>
        <p class="muted small">удерживай — отпустишь раньше {{ HOLD_MS }} мс, ничего не произойдёт.</p>
        <p v-if="!props.gate.may_return && props.gate.return_denied" class="denied small">
          вернуть нельзя: {{ props.gate.return_denied.text }}
        </p>
        <p v-else-if="rationaleHint('return', rationale, props.draft)" class="hint small">
          {{ rationaleHint("return", rationale, props.draft) }}
        </p>
      </div>
    </div>

    <label class="rationale">
      <span>Обоснование — пара слов, почему так решаешь (обязательно)</span>
      <textarea v-model="rationale" rows="2" placeholder="почему принимаю / почему возвращаю" />
    </label>

    <!-- Черновик возврата (REQ-SAY-05): приходит ЗАПОЛНЕННЫМ всеми видимыми
         замечаниями; каждый пункт правится и снимается одной кнопкой. -->
    <div class="draft">
      <div class="rowflex draft-head">
        <b>Черновик возврата</b>
        <span class="muted">
          — {{ props.draft.length ? `${props.draft.length} пункт(ов) уйдёт вместе с возвратом` : "пусто" }}
        </span>
        <div class="spacer" />
        <button v-if="props.draft.length" class="clear" @click="clearAll">снять все</button>
      </div>
      <p class="muted small" style="margin: 2px 0 0">{{ props.draftNote }}</p>
      <div v-for="d in props.draft" :key="d.key" class="draft-item">
        <span class="mono muted small draft-site">{{ siteText(d.site) }}</span>
        <input
          type="text"
          :value="d.requirement"
          placeholder="что нужно поправить"
          @input="setRequirement(d.key, ($event.target as HTMLInputElement).value)"
        />
        <button class="remove" @click="remove(d.key)">снять</button>
      </div>
      <p v-for="u in props.unchecked" :key="u.id" class="muted small unchecked-add">
        не проверено: {{ u.what }}
        <button v-if="!inDraft(props.draft, u.id)" class="add tiny" @click="addUnchecked(u)">
          добавить в черновик
        </button>
      </p>
    </div>

    <!-- Отказ платформы — РЯДОМ С КНОПКАМИ (REQ-QG-03 AC4/AC5): код и текст
         как есть, введённое обоснование и черновик не сбрасываются. -->
    <div v-if="props.error" class="gate-error">
      <span class="code">{{ props.error.code }}</span> {{ props.error.message }}
    </div>
  </div>
</template>

<style scoped>
.actions-wrap {
  margin-top: 10px;
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
  margin-top: 8px;
}
.draft-head {
  margin-bottom: 2px;
}
.draft-item {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
}
.draft-site {
  flex: 0 0 auto;
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.draft-item input {
  flex: 1;
  min-width: 120px;
}
.unchecked-add {
  margin: 4px 0 0;
}
.rationale {
  display: block;
  margin-top: 8px;
}
.rationale textarea {
  width: 100%;
  margin-top: 4px;
}
.actions {
  margin-top: 6px;
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
  white-space: normal;
  text-align: center;
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
.remove,
.tiny {
  font-size: 12px;
}
</style>
