<script setup lang="ts">
// Платформа спрашивает человека — блок над шагами, потому что без ответа
// работа не двинется. Гейт вопроса читается по событию `question.raised`,
// а не по формулировке: слово в тексте — текст, адрес перехода — данные.
import { computed, ref, watch } from "vue";
import type { Question, WorkflowEvent } from "../types";
import { GATE_REVIEW_RETURN, gateOf } from "./questions";
import { itemWord, type StatementReview } from "./statementReview";

const props = defineProps<{
  questions: Question[];
  events: WorkflowEvent[];
  busy?: string | null;
  /** Протокол проверки постановки: чек-лист и вопросы — поля, а не текст. */
  protocol?: StatementReview | null;
}>();
const emit = defineEmits<{ (e: "answer", q: Question, text: string): void }>();

const open = computed(() => props.questions.filter((q) => q.status === "OPEN"));
const freeText = ref<Record<string, string>>({});
// Ответы по КАЖДОМУ вопросу протокола: платформа получает один текст, но
// человек отвечает по пунктам — «одно поле на всё» заставляло его самого
// нумеровать вопросы и следить, на что он не ответил.
const answers = ref<string[]>([]);
watch(
  () => props.protocol?.questions.length ?? 0,
  (n) => {
    answers.value = Array.from({ length: n }, (_, i) => answers.value[i] ?? "");
  },
  { immediate: true },
);
const answeredCount = computed(() => answers.value.filter((a) => a.trim()).length);
const canSendProtocol = computed(() => answeredCount.value > 0);

/** Склейка ответов: вопрос остаётся рядом с ответом, чтобы читалось без сверки. */
function composeAnswers(p: StatementReview): string {
  return p.questions
    .map((q, i) => ({ q, a: (answers.value[i] ?? "").trim() }))
    .filter((x) => x.a.length)
    .map((x, k) => `${k + 1}) ${x.q}\nОтвет: ${x.a}`)
    .join("\n\n");
}

function sendProtocol(q: Question) {
  if (!props.protocol || !canSendProtocol.value) return;
  const text = composeAnswers(props.protocol);
  if (text) emit("answer", q, text);
}
const gate = (id: string) => gateOf(props.events, id);

function send(q: Question, text: string) {
  const value = text.trim();
  if (value) emit("answer", q, value);
}
</script>

<template>
  <div v-if="open.length" class="ask">
    <div class="ahead">платформа спрашивает · {{ open.length }}</div>

    <!-- протокол проверки: вердикт, чек-лист по пунктам, вопросы отдельно -->
    <div v-if="protocol" class="proto">
      <div class="pverdict" :class="protocol.verdict === 'APPROVED' ? 'ok' : 'no'">
        проверка постановки · {{ protocol.verdict === "APPROVED" ? "принята" : "есть вопросы" }}
      </div>
      <div v-if="protocol.human_paragraph" class="ppar">{{ protocol.human_paragraph }}</div>
      <div v-if="protocol.checklist.length" class="pchecks">
        <div v-for="c in protocol.checklist" :key="c.item" class="pcheck" :class="{ bad: !c.ok }">
          <span class="mk">{{ c.ok ? "✓" : "✗" }}</span>
          <span class="nm">{{ itemWord(c.item) }}</span>
          <span v-if="c.note" class="nt">{{ c.note }}</span>
        </div>
      </div>
      <div v-if="protocol.questions.length" class="pqs-h">
        что нужно решить · {{ protocol.questions.length }}
      </div>
    </div>
    <div v-for="q in open" :key="q.id" class="acard">
      <!-- если протокол разложен — сырой склеенный текст не дублируем:
           вердикт, чек-лист и вопросы уже выше -->
      <div v-if="!protocol" class="atext">{{ q.text }}</div>
      <div v-if="q.context && !protocol" class="actx">{{ q.context }}</div>

      <!-- вопрос гейта «ревью вернуло спеку»: два хода, оба — слово человека -->
      <div v-if="gate(q.id) === GATE_REVIEW_RETURN" class="abtns">
        <button class="a go" :disabled="busy === q.id" @click="send(q, 'продолжить')">Продолжить круг</button>
        <button class="a no" :disabled="busy === q.id" @click="send(q, 'остановить')">Остановить</button>
        <span class="ahint">продолжить — спека уйдёт аналитику; остановить — дело останется у тебя</span>
      </div>

      <!-- протокол: свой ответ на КАЖДЫЙ вопрос, отправка одним актом -->
      <template v-else-if="protocol && protocol.questions.length">
        <div v-for="(pq, i) in protocol.questions" :key="i" class="pq">
          <div class="pq-t"><span class="pq-n">{{ i + 1 }}</span>{{ pq }}</div>
          <textarea v-model="answers[i]" rows="2" spellcheck="false"
                    placeholder="твой ответ на этот вопрос" />
        </div>
        <div class="abtns">
          <button class="a go" :disabled="busy === q.id || !canSendProtocol" @click="sendProtocol(q)">
            Ответить
          </button>
          <span class="ahint">{{ answeredCount }} из {{ protocol.questions.length }} с ответом</span>
        </div>
      </template>

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

/* Протокол проверки постановки: поля, а не свободный текст. */
.proto{margin:2px 0 10px;border:1px solid var(--p-line);border-radius:9px;
  background:#0f141a;padding:10px 12px}
.pverdict{font-size:10.5px;letter-spacing:.09em;text-transform:uppercase;margin-bottom:7px}
.pverdict.ok{color:var(--p-pass)}
.pverdict.no{color:#ffa972}
.ppar{font-size:12.5px;line-height:1.55;color:#d7e0ea;margin-bottom:8px;white-space:pre-wrap}
.pchecks{display:flex;flex-direction:column;gap:3px;margin-bottom:8px}
.pcheck{display:flex;gap:8px;align-items:baseline;font-size:12px;color:#cfd9e4}
.pcheck .mk{flex:none;width:12px;color:var(--p-pass)}
.pcheck.bad .mk{color:var(--p-stop)}
.pcheck.bad .nm{color:#f2bcc2}
.pcheck .nm{flex:none;min-width:150px}
.pcheck .nt{color:var(--p-dim);font-size:11.5px}
.pqs-h{font-size:10.5px;letter-spacing:.09em;text-transform:uppercase;color:#93a1b1;margin-bottom:4px}
.pqs ol{margin:0;padding-left:18px;font-size:12.5px;line-height:1.55;color:#e2e9f1}
.pqs li{margin-bottom:3px}
.araw summary{cursor:pointer;font-size:10.5px;color:var(--p-dim2);margin-bottom:4px}
.araw[open] summary{color:var(--p-dim)}

/* Ответ по каждому вопросу протокола: вопрос с номером, под ним поле. */
.pq{margin-bottom:8px}
.pq-t{display:flex;gap:7px;font-size:12.5px;line-height:1.5;color:#e2e9f1;margin-bottom:4px}
.pq-n{flex:none;width:16px;color:#93a1b1;font-family:var(--mono);font-size:11px;padding-top:2px}
.pq textarea{width:100%;box-sizing:border-box;resize:vertical;min-height:44px;
  border:1px solid var(--p-line);border-radius:6px;background:#16202b;color:#f0f5fa;
  font-family:var(--sans);font-size:12.5px;line-height:1.5;padding:7px 9px}
.pq textarea::placeholder{color:#63707f}
.pq textarea:focus{outline:none;border-color:var(--p-human);background:#18242f}
</style>
