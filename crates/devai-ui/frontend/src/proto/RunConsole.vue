<script setup lang="ts">
// Консоль прогона как ЧТЕНИЕ, а не как дамп: роль отдаёт один длинный JSON,
// и он должен быть виден по местам — счётчики счётчиками, находки находками,
// абзацы абзацами. Сырой поток доступен отдельным тумблером: если разбор
// ошибается, человек видит ровно то, что на диске.
import { computed, ref } from "vue";

const props = defineProps<{ text: string }>();
const raw = ref(false);

type Chip = { k: string; v: string; cls: string };
type Block =
  | { kind: "chips"; key: string; chips: Chip[] }
  | { kind: "list"; key: string; items: { head: string; text: string }[] }
  | { kind: "kv"; key: string; value: string }
  | { kind: "prose"; key: string; text: string }
  | { kind: "json"; key: string; text: string };

function isPrim(v: unknown): boolean {
  return typeof v === "string" || typeof v === "number" || typeof v === "boolean";
}

/** Число — не «1.000000», а как его читает человек. */
function num(v: unknown): string {
  if (typeof v === "number") return Number.isInteger(v) ? String(v) : String(Math.round(v * 100) / 100);
  return String(v);
}

function chipCls(v: unknown): string {
  if (typeof v === "number") {
    if (v >= 1) return "ok";
    if (v >= 0.7) return "mid";
    return "bad";
  }
  if (typeof v === "boolean") return v ? "ok" : "bad";
  return "";
}

/** Многострочный текст роли хранится с экранированными переводами — их
 *  разворачиваем, иначе абзац читается одной строкой без смысла. */
function prose(v: string): string {
  return v.replace(/\\n/g, "\n").replace(/\s+$/g, "");
}

const HEAD_KEYS = ["check", "item", "name", "id", "kind", "title", "code"];
const TEXT_KEYS = ["text", "why", "message", "note", "human_paragraph", "detail", "reason"];

function listOf(arr: unknown[]): { head: string; text: string }[] {
  return arr.map((it, i) => {
    if (!it || typeof it !== "object") return { head: `${i + 1}`, text: String(it) };
    const o = it as Record<string, unknown>;
    const headKey = HEAD_KEYS.find((k) => typeof o[k] === "string") ?? "";
    const textKey = TEXT_KEYS.find((k) => typeof o[k] === "string" && k !== headKey) ?? "";
    const rest = Object.entries(o)
      .filter(([k]) => k !== headKey && k !== textKey)
      .map(([k, v]) => `${k}: ${isPrim(v) ? num(v) : JSON.stringify(v)}`)
      .join(" · ");
    return {
      head: headKey ? String(o[headKey]) : `${i + 1}`,
      text: textKey ? prose(String(o[textKey])) : rest,
    };
  });
}

const blocks = computed<Block[]>(() => {
  const text = props.text.trim();
  if (!text) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return [{ kind: "prose", key: "поток", text: props.text }];
  }
  if (!parsed || typeof parsed !== "object") {
    return [{ kind: "prose", key: "ответ", text: String(parsed) }];
  }
  if (Array.isArray(parsed)) {
    return [{ kind: "list", key: `записей ${parsed.length}`, items: listOf(parsed) }];
  }
  const out: Block[] = [];
  for (const [key, v] of Object.entries(parsed as Record<string, unknown>)) {
    if (Array.isArray(v)) {
      if (v.length && v.every((x) => x && typeof x === "object" && !Array.isArray(x))) {
        out.push({ kind: "list", key, items: listOf(v) });
      } else if (v.every(isPrim)) {
        out.push({
          kind: "chips",
          key,
          chips: v.map((x, i) => ({ k: String(i + 1), v: num(x), cls: chipCls(x) })),
        });
      } else {
        out.push({ kind: "json", key, text: JSON.stringify(v, null, 2) });
      }
    } else if (v && typeof v === "object") {
      const ent = Object.entries(v as Record<string, unknown>);
      const flat = ent.every(([, x]) => isPrim(x));
      if (flat) {
        out.push({
          kind: "chips",
          key,
          chips: ent.map(([k, x]) => ({ k, v: num(x), cls: chipCls(x) })),
        });
      } else {
        out.push({ kind: "json", key, text: JSON.stringify(v, null, 2) });
      }
    } else if (typeof v === "string") {
      const p = prose(v);
      if (p.includes("\n") || p.length > 140) out.push({ kind: "prose", key, text: p });
      else out.push({ kind: "kv", key, value: p });
    } else {
      out.push({ kind: "kv", key, value: num(v) });
    }
  }
  return out;
});

const hasStructure = computed(() => blocks.value.some((b) => b.kind !== "prose"));

/** Слово-вердикт читается цветом: «APPROVED» зелёный, «QUESTIONS» оранжевый —
 *  так же, как в остальной карточке, без отдельной легенды. */
function badgeCls(v: string): string {
  if (/^(APPROVED|PASSED|COMPLETED|OK|DONE)$/i.test(v)) return "good";
  if (/^(QUESTIONS|REJECTED|FAILED|ERROR|BLOCKED)$/i.test(v)) return "bad";
  return "";
}
</script>

<template>
  <div class="rc">
    <div v-if="hasStructure" class="rctop">
      <button class="rcset" :class="{ on: !raw }" @click="raw = false">по местам</button>
      <button class="rcset" :class="{ on: raw }" @click="raw = true">как есть</button>
    </div>

    <pre v-if="raw || !hasStructure" class="rcraw">{{ text }}</pre>

    <div v-else class="rcbody">
      <div v-for="(b, i) in blocks" :key="i" class="rcb">
        <div class="rck">{{ b.key }}</div>

        <div v-if="b.kind === 'chips'" class="rchips">
          <span v-for="(c, k) in b.chips" :key="k" class="rchip" :class="c.cls">
            <span class="ck">{{ c.k }}</span><span class="cv">{{ c.v }}</span>
          </span>
        </div>

        <ol v-else-if="b.kind === 'list'" class="rclist">
          <li v-for="(it, k) in b.items" :key="k">
            <span class="rh">{{ it.head }}</span>
            <span class="rt">{{ it.text }}</span>
          </li>
        </ol>

        <div v-else-if="b.kind === 'prose'" class="rcprose">{{ b.text }}</div>

        <div v-else-if="b.kind === 'kv'" class="rckv" :class="badgeCls(b.value)">{{ b.value }}</div>

        <pre v-else class="rcraw">{{ b.text }}</pre>
      </div>
    </div>
  </div>
</template>

<style scoped>
.rc{font-size:12px;color:#cfd9e4}
.rctop{display:flex;gap:4px;justify-content:flex-end;margin-bottom:6px}
.rcset{font:inherit;font-size:10.5px;padding:2px 8px;border-radius:5px;cursor:pointer;
  border:1px solid #222c37;background:#111821;color:#8b97a6}
.rcset.on{color:#e9eef4;border-color:#3a4756;background:#1a232d}
.rcbody{display:flex;flex-direction:column;gap:12px}
.rcb>.rck{font-size:10px;letter-spacing:.09em;text-transform:uppercase;color:#7f8c9b;margin-bottom:5px}
.rchips{display:flex;flex-wrap:wrap;gap:4px}
.rchip{display:inline-flex;gap:6px;align-items:baseline;padding:2px 7px;border-radius:5px;
  background:#111821;border:1px solid #1e2833;font-family:var(--mono);font-size:10.5px}
.rchip .ck{color:#8b97a6}
.rchip .cv{color:#e9eef4}
.rchip.ok .cv{color:#3ecf8e}
.rchip.mid .cv{color:#f0a44a}
.rchip.bad .cv{color:#f0616d}
.rclist{margin:0;padding:0;list-style:none;display:flex;flex-direction:column;gap:7px}
.rclist li{display:flex;gap:9px;align-items:baseline;border-left:2px solid #1e2833;padding-left:9px}
.rclist .rh{flex:none;font-family:var(--mono);font-size:10.5px;color:#7aa7ff;min-width:170px}
.rclist .rt{color:#dfe7f0;line-height:1.5;white-space:pre-wrap}
.rcprose{white-space:pre-wrap;line-height:1.6;color:#dfe7f0;max-width:110ch}
.rckv{font-family:var(--mono);font-size:11.5px;color:#dfe7f0;white-space:pre-wrap}
.rckv.good{color:#3ecf8e;font-weight:600}
.rckv.bad{color:#f0a44a;font-weight:600}
.rcraw{font-family:var(--mono);font-size:11px;line-height:1.5;color:#b8c4d2;white-space:pre-wrap;
  word-break:break-word;margin:0}
</style>
