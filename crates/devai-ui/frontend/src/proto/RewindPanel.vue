<script setup lang="ts">
// «Ошиблись на шаге» — человеческий ответ на ошибку пути.
//
// Два акта, оба — слово человека, оба требуют причины (в журнале должно быть
// видно, зачем путь пройден дважды): перепройти с названной стадии и переписать
// изначальную постановку. Платформа не удаляет результаты — она помечает их
// несвежими, поэтому здесь нет ни слова «удалить».
import { computed, ref } from "vue";

const props = defineProps<{
  state: string;
  title: string;
  description: string;
  busy?: boolean;
}>();
const emit = defineEmits<{
  (e: "rewind", to: string, reason: string): void;
  (e: "restate", title: string, description: string, reason: string): void;
}>();

// Стадии пути, с которых можно начать заново. Список — адреса объявления
// процесса (rewind_to_*); DRAFT/удержания целью не бывают.
const STAGES: [string, string][] = [
  ["STATEMENT", "постановка"],
  ["ANALYSIS", "анализ"],
  ["REVIEW", "ревью"],
  ["APPROVAL", "подпись"],
  ["IMPLEMENTATION", "реализация"],
  ["QA", "приёмка"],
];

const to = ref<string>(STAGES[0][0]);
const reason = ref("");
const newTitle = ref(props.title);
const newDescription = ref(props.description);

const reasonOk = computed(() => reason.value.trim().length > 0);
const canRewind = computed(() => reasonOk.value && to.value !== props.state && !props.busy);
const canRestate = computed(() => {
  const t = newTitle.value.trim();
  const d = newDescription.value;
  const changed = t !== props.title.trim() || d !== props.description;
  return reasonOk.value && t.length > 0 && changed && !props.busy;
});

function rewind() {
  if (canRewind.value) emit("rewind", to.value, reason.value.trim());
}
function restate() {
  if (canRestate.value) emit("restate", newTitle.value.trim(), newDescription.value, reason.value.trim());
}
</script>

<template>
  <div class="rw">
    <div class="rhead">
      шаг пройден по ошибке? вернись и пройди заново
      <span class="note">результаты не удаляются — помечаются несвежими</span>
    </div>

    <div class="rline">
      <span class="lab">с шага</span>
      <div class="chips">
        <button
          v-for="[v, word] in STAGES"
          :key="v"
          class="chip"
          :class="{ on: to === v, here: state === v }"
          :disabled="state === v || busy"
          :title="state === v ? 'задача уже здесь' : `вернуться к стадии «${word}»`"
          @click="to = v"
        >{{ word }}</button>
      </div>
    </div>

    <div class="rline">
      <span class="lab">почему</span>
      <input v-model="reason" placeholder="что было не так — это уйдёт в журнал" spellcheck="false" />
      <button class="go" :disabled="!canRewind" @click="rewind">Перепройти</button>
    </div>

    <div class="rsep"><span>или переписать постановку</span></div>

    <div class="rline top">
      <span class="lab">задача</span>
      <input v-model="newTitle" placeholder="название" spellcheck="false" />
    </div>
    <div class="rline top">
      <span class="lab">постановка</span>
      <textarea v-model="newDescription" rows="3" placeholder="как задача поставлена" spellcheck="false" />
      <button class="go" :disabled="!canRestate" @click="restate">Переписать</button>
    </div>
  </div>
</template>

<style scoped>
.rw{border:1px solid rgba(243,106,22,.5);border-radius:10px;background:rgba(243,106,22,.06);
  padding:10px 12px 11px;margin-bottom:12px}
.rhead{font-size:11px;letter-spacing:.04em;color:#ffa972;margin-bottom:8px;display:flex;gap:10px;flex-wrap:wrap}
.rhead .note{color:#93a1b1}
.rline{display:flex;align-items:center;gap:9px;margin-bottom:7px}
.rline.top{align-items:flex-start}
.rline .lab{flex:none;width:88px;font-size:10px;letter-spacing:.09em;text-transform:uppercase;color:var(--p-dim)}
.rline input,.rline textarea{flex:1;min-width:0;box-sizing:border-box;border:1px solid var(--p-line);
  border-radius:6px;background:#0b0e12;color:var(--p-ink);font-family:var(--sans);font-size:12.5px;padding:6px 9px}
.rline textarea{resize:vertical;line-height:1.45}
.rline input:focus,.rline textarea:focus{outline:none;border-color:var(--p-human)}
.chips{display:flex;gap:5px;flex-wrap:wrap}
.chip{border:1px solid var(--p-line);border-radius:5px;background:#121820;color:var(--p-dim);
  font-family:var(--sans);font-size:11.5px;padding:3px 9px;cursor:pointer}
.chip:hover:not(:disabled){color:var(--p-ink);border-color:#3a4753}
.chip.on{border-color:rgba(243,106,22,.8);background:rgba(243,106,22,.16);color:#ffc09a}
.chip.here{opacity:.4;cursor:not-allowed}
.go{border:1px solid rgba(243,106,22,.7);border-radius:6px;background:rgba(243,106,22,.18);
  color:#ffc09a;font-family:var(--sans);font-size:12px;padding:6px 13px;cursor:pointer;white-space:nowrap}
.go:hover:not(:disabled){background:rgba(243,106,22,.3)}
.go:disabled{opacity:.4;cursor:not-allowed}
.rsep{display:flex;align-items:center;gap:10px;margin:10px 0 8px;color:#93a1b1;font-size:10.5px}
.rsep::before,.rsep::after{content:"";flex:1;height:1px;background:var(--p-line)}
</style>
