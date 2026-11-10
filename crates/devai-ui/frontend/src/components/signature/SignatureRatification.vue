<script setup lang="ts">
// Блок «Решения, которые просят ратификации» (REQ-SIGN-03): каждый вопрос —
// пункт с полным сохранённым текстом, выбор — radio group из двух положений
// («принять как есть» по умолчанию, «свой ответ»). Компонент не хранит выбор:
// он сообщает номер и положение, строки обоснования собирает родитель.
import type { OqChoice } from "../../signature_view";

const props = defineProps<{
  questions: string[];
  choices: OqChoice[];
}>();

const emit = defineEmits<{
  (e: "choice", index: number, accept: boolean): void;
  (e: "answer", index: number, answer: string): void;
}>();
</script>

<template>
  <div class="ratification">
    <h4 class="subhead">Решения, которые просят ратификации ({{ props.questions.length }})</h4>
    <div v-for="(q, i) in props.questions" :key="i" class="oq">
      <div class="oq-text"><b>Вопрос {{ i + 1 }}.</b> {{ q }}</div>
      <div class="rowflex oq-choice">
        <label>
          <input
            type="radio"
            :name="`oq-${i}`"
            :checked="props.choices[i]?.accept !== false"
            @change="emit('choice', i, true)"
          />
          принять как есть
        </label>
        <label>
          <input
            type="radio"
            :name="`oq-${i}`"
            :checked="props.choices[i]?.accept === false"
            @change="emit('choice', i, false)"
          />
          свой ответ
        </label>
        <input
          v-if="props.choices[i] && !props.choices[i].accept"
          class="own-answer"
          :value="props.choices[i].answer"
          placeholder="свой ответ — обязателен"
          @input="emit('answer', i, ($event.target as HTMLInputElement).value)"
        />
      </div>
    </div>
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
/* Палец: каждая пара «радио + слово» — цель не меньше 44px по высоте,
   свой ответ занимает строку целиком. */
@media (max-width: 1199px) {
  .oq-choice label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 44px;
    padding-right: 8px;
  }
  .own-answer {
    width: 100%;
    min-width: 0;
  }
}
</style>
