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
            <span class="mark">вопрос {{ q.k + 1 }}</span>
            <span class="qt">{{ q.question }}</span>
          </div>
          <div class="qa">
            <span class="mark">твоё решение</span>
            <textarea
              v-model="rulings[q.k]"
              rows="2"
              spellcheck="false"
              placeholder="аналитик решения не предложил — впиши своё"
              @input="touched[q.k] = true"
              @keydown.ctrl.enter="sign(true)"
            />
            <span v-if="!touched[q.k] && q.proposal" class="hint">предложено аналитиком — правь как хочешь</span>
            <span v-else-if="touched[q.k]" class="hint edited">твоя редакция</span>
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
.sigsum .k{font-size:10px;letter-spacing:.09em;text-transform:uppercase;color:var(--p-dim2)}
.sigsum .v{font-family:var(--mono);font-size:17px;color:var(--p-ink);margin-top:2px}
.sigsum .n{font-size:10.5px;color:var(--p-dim);margin-top:1px}
.lead{font-size:13px;line-height:1.55;color:#d3dce6;max-width:120ch}
.sec{margin-top:12px;font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--p-dim2)}
.req{border:1px solid var(--p-line);border-radius:8px;padding:10px 12px;margin-bottom:8px;background:var(--p-card2)}
.req .rid{font-family:var(--mono);font-size:11px;color:var(--p-human)}
.req .rst{margin-top:4px;font-size:12.5px;color:#dbe3ec;line-height:1.45}
.ac{margin:6px 0 0;padding-left:16px;font-size:11.5px;color:var(--p-dim);line-height:1.5}
.oqcard{border:1px solid var(--p-line);border-radius:9px;margin-bottom:8px;background:var(--p-card2);overflow:hidden}
.oqcard .qq{display:flex;gap:9px;padding:9px 11px 8px;border-bottom:1px dashed var(--p-line)}
.oqcard .qa{display:flex;flex-direction:column;gap:5px;padding:8px 11px 9px;
  border-left:3px solid var(--p-human);background:rgba(122,167,255,.055)}
.oqcard .mark{flex:none;font-size:9.5px;letter-spacing:.09em;text-transform:uppercase;padding-top:2px}
.oqcard .qq .mark{color:var(--p-now)}
.oqcard .qa .mark{color:var(--p-human)}
.oqcard .qt{font-size:12.5px;line-height:1.5;color:#cfd9e4}
.oqcard textarea{width:100%;box-sizing:border-box;resize:vertical;min-height:38px;
  border:1px solid var(--p-line);border-radius:6px;background:#0b0e12;color:var(--p-ink);
  font-family:var(--sans);font-size:12.5px;line-height:1.45;padding:6px 8px}
.oqcard textarea:focus{outline:none;border-color:var(--p-human);box-shadow:0 0 0 2px rgba(122,167,255,.14)}
.oqcard .hint{font-size:10px;color:var(--p-dim2)}
.oqcard .hint.edited{color:var(--p-pass)}
.signnote{margin:12px 0 0}
.signnote input{width:100%;box-sizing:border-box;border:1px solid var(--p-line);border-radius:7px;
  background:#0b0e12;color:var(--p-ink);font-family:var(--sans);font-size:12.5px;padding:7px 9px}
.signnote input:focus{outline:none;border-color:var(--p-human)}
.signacts{display:flex;align-items:center;gap:9px;margin-top:10px;padding-top:10px;border-top:1px solid var(--p-line)}
.act{border-radius:7px;border:1px solid var(--p-line);background:#131a22;color:var(--p-ink);
  font-family:var(--sans);font-size:12.5px;padding:7px 16px;cursor:pointer}
.act.go{border-color:rgba(62,207,142,.55);background:rgba(62,207,142,.13);color:#8ee7bc;font-weight:600}
.act.go:hover:not(:disabled){background:rgba(62,207,142,.22)}
.act.no{border-color:rgba(240,97,109,.5);background:rgba(240,97,109,.1);color:#f4a3ab}
.act.no:hover:not(:disabled){background:rgba(240,97,109,.2)}
.act:disabled{opacity:.42;cursor:not-allowed}
.why{font-size:10.5px;color:var(--p-dim2)}
</style>
