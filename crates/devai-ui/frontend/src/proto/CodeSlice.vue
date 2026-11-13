<script setup lang="ts">
// Кусок исходника по ссылке «путь:строки» из отчёта: вставка кода на
// зафиксированном коммите. Читает платформа (source.slice), окно только
// раскладывает и подсвечивает. Контекст файла разворачивается вверх и вниз
// отдельными запросами того же чтения — вплоть до всего файла.
import { computed, onMounted, ref, watch } from "vue";
import Prism from "prismjs";
import "prismjs/components/prism-rust";
import "prismjs/components/prism-typescript";
import "prismjs/components/prism-json";
import "prismjs/components/prism-bash";
import "prismjs/components/prism-yaml";
import "prismjs/components/prism-toml";
import "prismjs/components/prism-markup";
import "prismjs/components/prism-markup-templating";
import "prismjs/components/prism-sql";
import "prismjs/components/prism-ini";
import "prismjs/plugins/line-numbers/prism-line-numbers";
import { api } from "../api";
import type { SourceSliceView } from "../types";

const props = defineProps<{
  taskId: string;
  rev: string;
  path: string;
  from: number;
  to: number;
}>();

const slice = ref<SourceSliceView | null>(null);
const failed = ref<string | null>(null);
const code = ref<HTMLElement | null>(null);
const step = 40; // сколько строк добавляет одно нажатие

// Грамматика — по расширению файла; неизвестное расширение просто без подсветки.
const GRAMMAR: Record<string, string> = {
  rs: "rust", ts: "typescript", tsx: "typescript", js: "typescript",
  json: "json", yaml: "yaml", yml: "yaml", toml: "toml", sh: "bash",
  bash: "bash", sql: "sql", html: "markup", vue: "markup", md: "markup",
  ini: "ini", conf: "ini",
};

function grammarFor(path: string): Prism.Grammar | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return Prism.languages[GRAMMAR[ext] ?? ""] ?? null;
}

function paint() {
  const el = code.value;
  if (!el || !slice.value) return;
  // Строки кладём как текст и отдаём Prism: highlightElement прогоняет
  // плагин нумерации (гуттер с реальными номерами из data-start).
  el.className = "language-" + (GRAMMAR[props.path.split(".").pop()?.toLowerCase() ?? ""] ?? "none");
  el.textContent = slice.value.text;
  if (grammarFor(props.path)) Prism.highlightElement(el);
}

// Первый показ — ровно то, о чём говорит ссылка; контекст — по кнопкам.
const total = computed(() => slice.value?.total_lines ?? 0);
const canUp = computed(() => !!slice.value && slice.value.from > 1);
const canDown = computed(() => !!slice.value && total.value > 0 && slice.value.to < total.value);
const isWhole = computed(
  () => !!slice.value && slice.value.from <= 1 && total.value > 0 && slice.value.to >= total.value,
);

async function load(from: number, to: number) {
  failed.value = null;
  try {
    slice.value = await api.sourceSlice(props.taskId, props.rev, props.path, from, to);
    // ждём, пока Vue вставит <code>, и подсвечиваем
    requestAnimationFrame(paint);
  } catch (e) {
    failed.value = e instanceof Error ? e.message : String(e);
  }
}

function loadLinked() {
  void load(props.from, props.to);
}
function moreUp() {
  if (slice.value) void load(Math.max(1, slice.value.from - step), slice.value.to);
}
function moreDown() {
  if (slice.value) void load(slice.value.from, slice.value.to + step);
}
function whole() {
  if (slice.value) void load(1, Math.max(slice.value.to, total.value));
}

onMounted(loadLinked);
watch(() => [props.path, props.from, props.to], loadLinked);

const lineCount = () => (slice.value?.text ?? "").split("\n").length;
</script>

<template>
  <div class="codeblock">
    <div class="cap">
      <span>{{ path }}:{{ slice?.from ?? from }}{{ (slice?.to ?? to) > (slice?.from ?? from) ? `–${slice?.to ?? to}` : "" }}</span>
      <span v-if="slice" class="n">{{ lineCount() }} стр<template v-if="total"> из {{ total }}</template></span>
      <span v-else-if="failed" class="n">код недоступен</span>
      <span v-else class="n">читаю…</span>
    </div>
    <pre v-if="slice" class="line-numbers" :data-start="slice.from"><code
      ref="code" :class="'language-' + (GRAMMAR[path.split('.').pop()?.toLowerCase() ?? ''] ?? 'none')"
    /></pre>
    <div v-if="slice && (canUp || canDown || !isWhole)" class="more">
      <button v-if="canUp" @click="moreUp">↑ ещё {{ step }} выше</button>
      <button v-if="canDown" @click="moreDown">↓ ещё {{ step }} ниже</button>
      <button v-if="total && !isWhole" @click="whole">развернуть весь файл</button>
      <span class="muted">{{ failed }}</span>
    </div>
  </div>
</template>

<style scoped>
.codeblock{border:1px solid var(--p-line);border-radius:8px;overflow:hidden;margin:8px 0;background:#0b0e12}
.cap{display:flex;justify-content:space-between;gap:10px;padding:5px 10px;border-bottom:1px solid var(--p-line);
  background:#0f1318;font-family:var(--mono);font-size:11px;color:var(--p-dim2)}
.cap .n{white-space:nowrap}
pre{margin:0;padding:9px 12px;overflow:auto;font-family:var(--mono);font-size:11.5px;line-height:1.55;max-height:520px}
.more{display:flex;gap:8px;align-items:center;padding:6px 10px;border-top:1px solid var(--p-line);background:#0e141a}
.more button{background:transparent;border:1px solid var(--p-line);color:var(--p-human);border-radius:6px;
  padding:2px 8px;font-size:11px;cursor:pointer}
.more button:hover{background:rgba(122,167,255,.1)}
.muted{font-size:11px;color:var(--p-stop)}
code{font-family:inherit;font-size:inherit;color:#c5d0dc;background:none}
@media (max-width: 1199px){.cap{font-size:12px}pre{font-size:12px}}
</style>
