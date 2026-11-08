<script setup lang="ts">
// Секция «Спека на утверждение» (REQ-SIGN-01..04): абзац целиком, требования
// с раскрываемыми критериями, блок ратификации с radio-выбором на каждый
// вопрос документа, видимое поле обоснования и кнопки подписи.
//
// Никаких модальных окон: подпись — удержанием самой кнопки (hold-to-confirm);
// одиночный/двойной клик и отпускание раньше HOLD_MS мутацию не отправляют.
// Никакого скрытого payload: всё, что уйдёт в rationale, оператор видит в
// правимом поле. Права подписи — только may_approve/may_reject из ответа
// task.signature; UI их не пересчитывает.
import { computed, ref, watch } from "vue";
import type { SignatureView } from "../types";
import {
  HOLD_MS,
  choicesValid,
  criteriaSummary,
  denialText,
  holdCompletes,
  mergeOqLines,
  rationaleValid,
  reviewParagraph,
  reviewVerdict,
  specParagraph,
  type OqChoice,
} from "../signature_view";
import { labelFor, ARTIFACT_STATUS_LABELS, SIGNATURE_KIND_LABELS } from "../signature_labels";

const props = defineProps<{
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
// положение по умолчанию (ASM-6).
watch(
  () => props.signature.spec?.open_questions,
  (questions) => {
    choices.value = (questions ?? []).map(() => ({ accept: true, answer: "" }));
    rationale.value = "";
  },
  { immediate: true },
);

const spec = computed(() => props.signature.spec);
const paragraph = computed(() => (spec.value ? specParagraph(spec.value) : null));
const review = computed(() => props.signature.review);
const reviewText = computed(() => (review.value ? reviewParagraph(review.value) : null));
const verdict = computed(() => (review.value ? reviewVerdict(review.value) : null));

const approvalDenied = computed(() => denialText(props.signature.may_approve_denied));
const rejectDenied = computed(() => denialText(props.signature.may_reject_denied));

const canSign = computed(
  () => rationaleValid(rationale.value) && choicesValid(choices.value) && !props.busy,
);

function onChoice(index: number, accept: boolean) {
  choices.value[index] = { ...choices.value[index], accept };
  rationale.value = mergeOqLines(rationale.value, choices.value);
}
function onAnswer(index: number, answer: string) {
  choices.value[index] = { ...choices.value[index], answer };
  rationale.value = mergeOqLines(rationale.value, choices.value);
}

// --- hold-to-confirm ------------------------------------------------------
type Holding = "approve" | "reject" | null;
const holding = ref<Holding>(null);
const progress = ref(0);
let timer: ReturnType<typeof setTimeout> | null = null;
let raf = 0;
let startedAt = 0;

function stop() {
  if (timer) clearTimeout(timer);
  timer = null;
  if (raf) cancelAnimationFrame(raf);
  raf = 0;
  holding.value = null;
  progress.value = 0;
}

function start(kind: "approve" | "reject", event: PointerEvent) {
  if (!canSign.value) return;
  // Одиночный клик не срабатывает: обработчика click нет вообще.
  stop();
  (event.currentTarget as HTMLElement | null)?.setPointerCapture?.(event.pointerId);
  holding.value = kind;
  startedAt = performance.now();
  const tick = () => {
    const elapsed = performance.now() - startedAt;
    progress.value = Math.min(1, elapsed / HOLD_MS);
    if (holdCompletes(elapsed)) {
      const text = rationale.value;
      stop();
      emit("apply", kind, text);
      return;
    }
    raf = requestAnimationFrame(tick);
  };
  timer = setTimeout(() => undefined, HOLD_MS);
  raf = requestAnimationFrame(tick);
}

function release() {
  // Отпускание раньше порога: мутации нет.
  stop();
}
</script>

<template>
  <div class="panel signature">
    <h3>
      Спека на утверждение
      <template v-if="spec">
        · версия {{ spec.artifact.seq }}
        <span class="muted mono" style="font-weight: 400"> {{ spec.artifact.name }}</span>
      </template>
      <span class="spacer" />
      <a
        v-if="signature.previous"
        href="#spec-diff"
        class="muted diff-link"
        @click.prevent="emit('open-diff')"
      >дифф к прошлой версии</a>
      <span v-else class="muted">{{ signature.previous_note ?? "прошлой версии нет" }}</span>
    </h3>
    <div class="body">
      <!-- spec=null: утверждать нечего, но отклонение может быть доступно -->
      <p v-if="!spec" class="muted" style="margin: 0 0 10px">
        У задачи не опубликовано ни одного спецификационного артефакта —
        <b>утверждать нечего</b>.
      </p>

      <template v-else>
        <p v-if="!spec.current" class="notice">
          показана последняя спека, но она <b>не CURRENT</b> (версия
          {{ spec.artifact.seq }}) — утвердить нельзя, отклонить можно.
        </p>

        <!-- Абзац целиком, до формальной структуры -->
        <p v-if="paragraph" class="muted" style="margin: 0 0 4px">
          {{ paragraph.sourceNote }}
          <template v-if="paragraph.note">· {{ paragraph.note }}</template>
        </p>
        <p v-if="paragraph" class="paragraph">{{ paragraph.text }}</p>

        <div class="rowflex meta">
          <span class="muted">статус спеки:</span>
          <span>{{ labelFor(ARTIFACT_STATUS_LABELS, spec.artifact.status).text }}</span>
          <span v-if="spec.current" class="muted">· CURRENT</span>
        </div>

        <!-- Требования: id вторично и приглушённо -->
        <h4 class="subhead">Требования ({{ spec.requirements.length }})</h4>
        <p v-if="spec.requirements_note" class="muted" style="margin: 0 0 6px">
          {{ spec.requirements_note }}
        </p>
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
              <li v-for="(c, j) in r.constraints" :key="`c-${j}`" class="muted">ограничение: {{ c }}</li>
            </ul>
          </details>
        </div>
        <p v-if="!spec.requirements.length && !spec.requirements_note" class="muted" style="margin: 0">
          требований в спеке нет
        </p>

        <!-- Решения, которые просят ратификации -->
        <h4 v-if="spec.open_questions.length" class="subhead">
          Решения, которые просят ратификации ({{ spec.open_questions.length }})
        </h4>
        <div v-for="(q, i) in spec.open_questions" :key="i" class="oq">
          <div class="oq-text"><b>Вопрос {{ i + 1 }}.</b> {{ q }}</div>
          <div class="rowflex oq-choice">
            <label>
              <input
                type="radio"
                :name="`oq-${i}`"
                :checked="choices[i]?.accept !== false"
                @change="onChoice(i, true)"
              />
              принять как есть
            </label>
            <label>
              <input
                type="radio"
                :name="`oq-${i}`"
                :checked="choices[i]?.accept === false"
                @change="onChoice(i, false)"
              />
              свой ответ
            </label>
            <input
              v-if="choices[i] && !choices[i].accept"
              class="own-answer"
              :value="choices[i].answer"
              placeholder="свой ответ — обязателен"
              @input="onAnswer(i, ($event.target as HTMLInputElement).value)"
            />
          </div>
        </div>

        <!-- Ревьювер: вердикт сохранённым токеном + абзац -->
        <div class="reviewer">
          <h4 class="subhead" style="margin-top: 12px">
            Отчёт ревьювера
            <template v-if="review">
              · версия {{ review.artifact.seq }}
              <span v-if="!review.current" class="muted">(не CURRENT)</span>
            </template>
          </h4>
          <template v-if="review">
            <div class="rowflex">
              <span class="mono">вердикт: {{ verdict?.token }}</span>
              <span class="muted">{{ verdict?.word }}</span>
            </div>
            <p class="muted small" style="margin: 2px 0 4px">
              {{ reviewText?.sourceNote }}
              <template v-if="reviewText?.note">· {{ reviewText.note }}</template>
            </p>
            <p class="paragraph">{{ reviewText?.text }}</p>
          </template>
          <p v-else class="muted">{{ signature.review_note ?? "отчёта ревьювера нет" }}</p>
        </div>
      </template>

      <!-- Обоснование: видимое правимое поле, то, что уходит в решение -->
      <label class="rationale">
        <span>Обоснование подписи (обязательно — уходит в решение как rationale)</span>
        <textarea
          v-model="rationale"
          rows="4"
          placeholder="почему подписываю / почему отклоняю"
        />
      </label>
      <p v-if="!rationaleValid(rationale)" class="muted small" style="margin: 4px 0 0">
        без непустого обоснования подпись недоступна.
      </p>
      <p v-else-if="!choicesValid(choices)" class="notice small" style="margin: 4px 0 0">
        у каждого «своего ответа» на вопрос должен быть непустой текст.
      </p>

      <div class="rowflex actions">
        <button
          v-if="signature.may_approve"
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
          <span class="label">Утвердить — удерживайте {{ HOLD_MS }} мс</span>
        </button>
        <span v-else class="muted">
          «Утвердить» недоступно: {{ approvalDenied ?? "may_approve=false" }}
        </span>

        <button
          v-if="signature.may_reject"
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
          «Отклонить» недоступно: {{ rejectDenied ?? "may_reject=false" }}
        </span>
      </div>
      <p v-if="signature.may_reject" class="notice small" style="margin: 8px 0 0">
        Отклонение переведёт задачу <b>APPROVAL → ANALYSIS</b>; кнопка срабатывает
        только удержанием, одиночный клик ничего не делает.
      </p>

      <p v-if="signature.signature" class="muted small" style="margin: 10px 0 0">
        уже подписана: {{ labelFor(SIGNATURE_KIND_LABELS, signature.signature.kind).text }}
        — {{ signature.signature.actor }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.signature .paragraph {
  margin: 0 0 10px;
  white-space: pre-wrap;
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
.oq {
  margin-bottom: 10px;
}
.oq-text {
  white-space: pre-wrap;
  margin-bottom: 4px;
}
.oq-choice {
  gap: 14px;
  align-items: center;
}
.own-answer {
  flex: 1;
  min-width: 180px;
}
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
.notice {
  border-left: 3px solid var(--orange);
  padding: 4px 8px;
  color: var(--orange);
}
.small {
  font-size: 12px;
}
.diff-link {
  font-weight: 400;
  font-size: 12px;
}
</style>
