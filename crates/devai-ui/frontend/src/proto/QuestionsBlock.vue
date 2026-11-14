<script setup lang="ts">
// Платформа спрашивает человека — блок над шагами, потому что без ответа
// работа не двинется. Гейт вопроса читается по событию `question.raised`,
// а не по формулировке: слово в тексте — текст, адрес перехода — данные.
import { computed, ref } from "vue";
import type { Question, WorkflowEvent } from "../types";
import { GATE_REVIEW_RETURN, gateOf } from "./questions";

const props = defineProps<{
  questions: Question[];
  events: WorkflowEvent[];
  busy?: string | null;
}>();
const emit = defineEmits<{ (e: "answer", q: Question, text: string): void }>();

const open = computed(() => props.questions.filter((q) => q.status === "OPEN"));
const freeText = ref<Record<string, string>>({});
const gate = (id: string) => gateOf(props.events, id);

function send(q: Question, text: string) {
  const value = text.trim();
  if (value) emit("answer", q, value);
}
</script>

<template>
  <div v-if="open.length" class="ask">
    <div class="ahead">платформа спрашивает · {{ open.length }}</div>
    <div v-for="q in open" :key="q.id" class="acard">
      <div class="atext">{{ q.text }}</div>
      <div v-if="q.context" class="actx">{{ q.context }}</div>

      <!-- вопрос гейта «ревью вернуло спеку»: два хода, оба — слово человека -->
      <div v-if="gate(q.id) === GATE_REVIEW_RETURN" class="abtns">
        <button class="a go" :disabled="busy === q.id" @click="send(q, 'продолжить')">Продолжить круг</button>
        <button class="a no" :disabled="busy === q.id" @click="send(q, 'остановить')">Остановить</button>
        <span class="ahint">продолжить — спека уйдёт аналитику; остановить — дело останется у тебя</span>
      </div>

      <!-- прочие вопросы: свободный ответ -->
      <div v-else class="afree">
        <input v-model="freeText[q.id]" placeholder="ответ" spellcheck="false"
               @keydown.enter="send(q, freeText[q.id] ?? '')" />
        <button class="a" :disabled="busy === q.id || !(freeText[q.id] ?? '').trim()"
                @click="send(q, freeText[q.id] ?? '')">Ответить</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ask{border:1px solid rgba(243,106,22,.55);border-radius:10px;background:rgba(243,106,22,.07);
  padding:10px 12px 11px;margin-bottom:12px}
.ahead{font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#ffa972;margin-bottom:8px}
.acard{border-top:1px solid rgba(243,106,22,.25);padding-top:8px;margin-top:8px}
.acard:first-of-type{border-top:0;padding-top:0;margin-top:0}
.atext{font-size:13px;line-height:1.5;color:#e7edf4;white-space:pre-wrap}
.actx{margin-top:3px;font-family:var(--mono);font-size:10.5px;color:#93a1b1;white-space:pre-wrap}
.abtns{display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-top:8px}
.afree{display:flex;gap:8px;margin-top:8px}
.afree input{flex:1;min-width:0;box-sizing:border-box;border:1px solid var(--p-line);border-radius:6px;
  background:#0b0e12;color:var(--p-ink);font-family:var(--sans);font-size:12.5px;padding:6px 9px}
.afree input:focus{outline:none;border-color:var(--p-human)}
.a{border-radius:6px;border:1px solid var(--p-line);background:#131a22;color:var(--p-ink);
  font-family:var(--sans);font-size:12.5px;padding:6px 13px;cursor:pointer}
.a.go{border-color:#3ecf8e;background:rgba(62,207,142,.2);color:#b6f2d5;font-weight:600}
.a.no{border-color:rgba(240,97,109,.75);background:rgba(240,97,109,.16);color:#f6bcc2}
.a:disabled{opacity:.45;cursor:not-allowed}
.ahint{font-size:10.5px;color:#93a1b1}
</style>
