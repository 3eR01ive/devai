<script setup lang="ts">
// Текст с ссылками на код: ссылка «путь:строки» рисуется чипом, а вставка кода
// под ней СВЁРНУТА и открывается кликом. Так и в замечаниях проверки, и в
// свидетельствах приёмки — один компонент, одно правило.
import { ref } from "vue";
import CodeSlice from "./CodeSlice.vue";

const props = defineProps<{
  text: string;
  taskId: string;
  rev: string;
}>();

// Ссылки бывают в виде `путь:12`, `путь:12-20` и с ведущим слэшем.
const REF = /(?:^|[\s(«"'])([\w./-]+\.(?:rs|ts|tsx|js|vue|json|yaml|yml|toml|sql|md|py|sh)):(\d+)(?:[-–](\d+))?/g;

type Piece =
  | { t: "text"; v: string }
  | { t: "code"; path: string; from: number; to: number };

function pieces(raw: unknown): Piece[] {
  // Компонент принимает ТЕКСТ: объект на входе — ошибка вызывающего, но
  // печатать «[object Object]» вместо формулировки нельзя.
  const text = typeof raw === "string" ? raw : raw == null ? "" : String((raw as any).text ?? JSON.stringify(raw));
  const out: Piece[] = [];
  let last = 0;
  for (const m of text.matchAll(REF)) {
    const [raw, path, a, b] = m;
    const start = (m.index ?? 0) + (raw.startsWith(path ?? "") ? 0 : raw.indexOf(path ?? ""));
    if (start < last) continue;
    if (start > last) out.push({ t: "text", v: text.slice(last, start) });
    out.push({
      t: "code",
      path: String(path ?? ""),
      from: a ? Number(a) : 1,
      to: b ? Number(b) : a ? Number(a) + 11 : 12,
    });
    last = start + String(path ?? "").length + String(a ?? "").length + 2 + (b ? String(b).length : 0);
  }
  if (last < text.length) out.push({ t: "text", v: text.slice(last) });
  return out;
}

const open = ref<Record<number, boolean>>({});
function toggle(i: number) {
  open.value = { ...open.value, [i]: !open.value[i] };
}
</script>

<template>
  <div class="reftext">
    <template v-for="(p, i) in pieces(props.text)" :key="i">
      <template v-if="p.t === 'text'">{{ p.v }}</template>
      <template v-else>
        <button class="p-coderef" :class="{ on: open[i] }" @click.stop="toggle(i)">
          {{ p.path }}:{{ p.from }}{{ p.to > p.from ? "–" + p.to : "" }}
        </button>
        <template v-if="open[i]">
          <CodeSlice
            v-if="rev"
            :task-id="taskId"
            :rev="rev"
            :path="p.path"
            :from="p.from"
            :to="p.to"
          />
          <div v-else class="refnone">код недоступен: у задачи нет зафиксированного коммита</div>
        </template>
      </template>
    </template>
  </div>
</template>

<style scoped>
.reftext{font-size:12.5px;line-height:1.5;color:#dbe3ec}
.p-coderef{background:rgba(122,167,255,.10);border:1px solid rgba(122,167,255,.35);
  color:#7aa7ff;border-radius:4px;padding:0 5px;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;
  font-size:11px;cursor:pointer;margin:0 2px}
.p-coderef:hover{background:rgba(122,167,255,.22)}
.p-coderef.on{background:rgba(62,207,142,.2);border-color:rgba(62,207,142,.6);color:#8fe0b7}
.refnone{font-size:11px;color:#8b97a6;margin:4px 0}
</style>
