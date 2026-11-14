<script setup lang="ts">
// Гейт подписи спеки: что подписывают, вопросы и два акта.
//
// Вопрос в документе — текст с предложением аналитика («Предлагаю: …»).
// Человек не сочиняет заново: он правит предложенное, и решения уходят в
// подпись одной строкой на вопрос. Вопрос и решение разделены визуально:
// вопрос читают, решение правят.
import { computed, ref, watch } from "vue";
import type { SignatureView } from "../types";
import { buildRationale, oqParts } from "./signText";

const props = defineProps<{ signature: SignatureView | null; busy?: boolean }>();
const emit = defineEmits<{ (e: "sign", approve: boolean, rationale: string | null): void }>();

const oqList = computed(() =>
  (props.signature?.spec?.open_questions ?? []).map((q, k) => ({ k, ...oqParts(q) })),
);

// Черновик решения по каждому вопросу: что человек поправил, то и подпишется.
const rulings = ref<Record<number, string>>({});
const touched = ref<Record<number, boolean>>({});
const note = ref("");

// Правка человека не затирается перечиткой: предложение подставляем только
// туда, где человек ещё ничего не написал сам.
watch(
  () => props.signature,
  () => {
    const next: Record<number, string> = {};
    oqList.value.forEach((q) => {
      next[q.k] = touched.value[q.k] ? (rulings.value[q.k] ?? "") : q.proposal;
    });
    rulings.value = next;
  },
  { immediate: true },
);

const rationale = computed(() => buildRationale(oqList.value, rulings.value, note.value));

function sign(approve: boolean) {
  emit("sign", approve, rationale.value);
}
</script>

<template>
  <div class="p-panel">
    <div v-if="!signature?.spec" class="empty">спеки нет — подписывать нечего</div>
    <template v-else>
      <!-- что именно подписывают: счёт, а не проза -->
      <div class="sigsum">
        <div class="cell"><div class="k">спека</div>
          <div class="v">v{{ signature.spec.artifact.seq }}</div>
          <div class="n">{{ signature.spec.artifact.status.toLowerCase() }}</div></div>
        <div class="cell"><div class="k">требований</div>
          <div class="v">{{ signature.spec.requirements.length }}</div>
          <div class="n">{{ signature.spec.requirements.reduce((s, r) => s + r.acceptance_criteria.length, 0) }} приёмок</div></div>
        <div class="cell"><div class="k">вопросов</div>
          <div class="v">{{ oqList.length }}</div>
          <div class="n">{{ oqList.filter(q => q.proposal).length }} с предложением</div></div>
        <div class="cell"><div class="k">подпись</div>
          <div class="v">{{ signature.signature ? (signature.signature.kind === "approval" ? "есть" : "отказ") : "нет" }}</div>
          <div class="n">{{ signature.signature ? "слово человека" : "ждёт тебя" }}</div></div>
      </div>

      <div v-if="signature.spec.human_paragraph" class="lead">
        {{ signature.spec.human_paragraph }}
      </div>

      <div class="sec">требований · {{ signature.spec.requirements.length }}</div>
      <div v-for="r in signature.spec.requirements" :key="r.id" class="req">
        <div class="rid">{{ r.id }}</div>
        <div class="rst">{{ r.statement }}</div>
        <ul v-if="r.acceptance_criteria.length" class="ac">
          <li v-for="(a, k) in r.acceptance_criteria" :key="k">{{ a }}</li>
        </ul>
      </div>

      <!-- вопрос и решение разделены: вопрос читают, решение правят -->
      <template v-if="oqList.length">
        <div class="sec">вопросы, которые решаешь ты · {{ oqList.length }}</div>
        <div v-for="q in oqList" :key="'oq' + q.k" class="oqcard">
          <div class="qq">
            <span class="pill q">вопрос {{ q.k + 1 }}</span>
            <span class="qt">{{ q.question }}</span>
          </div>
          <div class="qa">
            <div class="qahead">
              <span class="pill a">твоё решение</span>
              <span v-if="!touched[q.k] && q.proposal" class="src">предложено аналитиком</span>
              <span v-else-if="touched[q.k]" class="src edited">твоя редакция</span>
              <span v-else class="src empty">решения нет — впиши своё</span>
            </div>
            <textarea
              v-model="rulings[q.k]"
              rows="2"
              spellcheck="false"
              placeholder="напиши решение по этому вопросу"
              @input="touched[q.k] = true"
              @keydown.ctrl.enter="sign(true)"
            />
          </div>
        </div>
      </template>

      <div class="signnote">
        <input v-model="note" placeholder="комментарий к подписи (необязательно)" />
      </div>

      <div class="signacts">
        <button class="act go" :disabled="busy || !signature.may_approve"
                :title="signature.may_approve_denied ?? 'подписать спеку и открыть реализацию'"
                @click="sign(true)">Утвердить</button>
        <button class="act no" :disabled="busy || !signature.may_reject"
                :title="signature.may_reject_denied ?? 'вернуть спеку аналитику'"
                @click="sign(false)">Отклонить</button>
        <span class="why">
          <template v-if="!signature.may_approve && signature.may_approve_denied">{{ signature.may_approve_denied }}</template>
          <template v-else-if="busy">отправляю…</template>
          <template v-else>решения по вопросам уйдут в подпись</template>
        </span>
      </div>
    </template>
  </div>
</template>

<style scoped>
.empty{padding:4px 0;color:var(--p-dim)}
.sigsum{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));border:1px solid var(--p-line);
  border-radius:10px;background:var(--p-card2);margin-bottom:12px;overflow:hidden}
.sigsum .cell{padding:9px 12px;border-right:1px solid var(--p-line)}
.sigsum .cell:last-child{border-right:0}
.sigsum .k{font-size:10px;letter-spacing:.09em;text-transform:uppercase;color:#93a1b1}
.sigsum .v{font-family:var(--mono);font-size:18px;color:var(--p-ink);margin-top:3px}
.sigsum .n{font-size:11px;color:#9fb0c2;margin-top:2px}
.lead{font-size:13px;line-height:1.55;color:#d3dce6;max-width:120ch}
.sec{margin:16px 0 8px;font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#93a1b1;
  padding-bottom:5px;border-bottom:1px solid var(--p-line)}
.req{border:1px solid var(--p-line);border-radius:8px;padding:10px 12px;margin-bottom:8px;background:var(--p-card2)}
.req .rid{font-family:var(--mono);font-size:11px;color:var(--p-human)}
.req .rst{margin-top:4px;font-size:12.5px;color:#dbe3ec;line-height:1.45}
.ac{margin:6px 0 0;padding-left:16px;font-size:11.5px;color:var(--p-dim);line-height:1.5}
/* Вопрос и решение — разные поверхности, а не два абзаца одного текста:
   вопрос на янтарной черте читают, решение на синей правят. */
.oqcard{border:1px solid var(--p-line);border-radius:9px;margin-bottom:9px;background:var(--p-card2);overflow:hidden}
.oqcard .qq{display:flex;gap:10px;align-items:flex-start;padding:10px 12px;
  border-left:3px solid var(--p-now);background:rgba(243,106,22,.045)}
.oqcard .qa{display:flex;flex-direction:column;gap:7px;padding:10px 12px 11px;
  border-left:3px solid var(--p-human);border-top:1px solid var(--p-line);
  background:rgba(122,167,255,.075)}
.pill{flex:none;border-radius:4px;padding:2px 7px;font-size:9.5px;letter-spacing:.09em;
  text-transform:uppercase;white-space:nowrap;line-height:1.5}
.pill.q{background:rgba(243,106,22,.16);border:1px solid rgba(243,106,22,.45);color:#ffa972}
.pill.a{background:rgba(122,167,255,.18);border:1px solid rgba(122,167,255,.5);color:#a9c6ff}
.oqcard .qt{font-size:13.5px;line-height:1.55;color:#e2e9f1}
.oqcard .qahead{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.oqcard .src{font-size:11px;color:#a9bccd;display:inline-flex;align-items:center;gap:5px}
.oqcard .src::before{content:"";width:6px;height:6px;border-radius:50%;background:#7aa7ff}
.oqcard .src.edited{color:#8ee7bc}
.oqcard .src.edited::before{background:#3ecf8e}
.oqcard .src.empty{color:#ffa972}
.oqcard .src.empty::before{background:#f36a16}
.oqcard textarea{width:100%;box-sizing:border-box;resize:vertical;min-height:50px;
  border:1px solid rgba(122,167,255,.35);border-radius:6px;background:#16202b;color:#f0f5fa;
  font-family:var(--sans);font-size:13px;line-height:1.5;padding:8px 10px}
.oqcard textarea::placeholder{color:#63707f}
.oqcard textarea:focus{outline:none;border-color:var(--p-human);box-shadow:0 0 0 2px rgba(122,167,255,.18);
  background:#18242f}
.signnote{margin:12px 0 0}
.signnote input{width:100%;box-sizing:border-box;border:1px solid var(--p-line);border-radius:7px;
  background:#0b0e12;color:var(--p-ink);font-family:var(--sans);font-size:12.5px;padding:7px 9px}
.signnote input:focus{outline:none;border-color:var(--p-human)}
.signacts{display:flex;align-items:center;gap:9px;margin-top:10px;padding-top:10px;border-top:1px solid var(--p-line)}
.act{border-radius:7px;border:1px solid var(--p-line);background:#131a22;color:var(--p-ink);
  font-family:var(--sans);font-size:13px;padding:8px 20px;cursor:pointer}
.act.go{border-color:#3ecf8e;background:rgba(62,207,142,.2);color:#b6f2d5;font-weight:600}
.act.go:hover:not(:disabled){background:rgba(62,207,142,.32)}
.act.no{border-color:rgba(240,97,109,.75);background:rgba(240,97,109,.16);color:#f6bcc2}
.act.no:hover:not(:disabled){background:rgba(240,97,109,.26)}
.act:disabled{opacity:.45;cursor:not-allowed}
.why{font-size:11px;color:#93a1b1}
</style>
