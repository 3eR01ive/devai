<script setup lang="ts">
// Кусок исходника по ссылке «путь:строки» из отчёта: вставка кода на
// зафиксированном коммите. Читает платформа (source.slice), окно только
// раскладывает и подсвечивает.
import { onMounted, ref, watch } from "vue";
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

async function load() {
  failed.value = null;
  try {
    slice.value = await api.sourceSlice(props.taskId, props.rev, props.path, props.from, props.to);
    // ждём, пока Vue вставит <code>, и подсвечиваем
    requestAnimationFrame(paint);
  } catch (e) {
    failed.value = e instanceof Error ? e.message : String(e);
  }
}

onMounted(load);
watch(() => [props.path, props.from, props.to], load);

const lineCount = () => (slice.value?.text ?? "").split("\n").length;
</script>

<template>
  <div class="codeblock">
    <div class="cap">
      <span>{{ path }}:{{ from }}{{ to > from ? `–${to}` : "" }}</span>
      <span v-if="slice" class="n">{{ lineCount() }} стр</span>
      <span v-else-if="failed" class="n">код недоступен</span>
      <span v-else class="n">читаю…</span>
    </div>
    <pre v-if="slice" class="line-numbers" :data-start="slice.from"><code
      ref="code" :class="'language-' + (GRAMMAR[path.split('.').pop()?.toLowerCase() ?? ''] ?? 'none')"
    /></pre>
  </div>
</template>

<style scoped>
.codeblock{border:1px solid var(--p-line);border-radius:8px;overflow:hidden;margin:8px 0;background:#0b0e12}
.cap{display:flex;justify-content:space-between;gap:10px;padding:5px 10px;border-bottom:1px solid var(--p-line);
  background:#0f1318;font-family:var(--mono);font-size:11px;color:var(--p-dim2)}
.cap .n{white-space:nowrap}
pre{margin:0;padding:9px 12px;overflow:auto;font-family:var(--mono);font-size:11.5px;line-height:1.55}
code{font-family:inherit;font-size:inherit;color:#c5d0dc;background:none}
@media (max-width: 1199px){.cap{font-size:12px}pre{font-size:12px}}
</style>
