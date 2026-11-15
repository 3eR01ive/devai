<script setup lang="ts">
// Выводы проверяющего постановку — отдельная вкладка шага «постановка».
// Поля контракта `statement-review@1`, а не свободный текст: вердикт, абзац
// человеку, чек-лист по шести пунктам и вопросы, которые он поднял.
import { itemWord, type StatementReview } from "./statementReview";

const props = defineProps<{ review: StatementReview | null }>();
</script>

<template>
  <div class="sv">
    <div v-if="!props.review" class="empty">
      проверки постановки ещё не было — постановщик не отдавал вердикта
    </div>
    <template v-else>
      <div class="head" :class="props.review.verdict === 'APPROVED' ? 'ok' : 'no'">
        <span class="vb">{{ props.review.verdict === "APPROVED" ? "принята" : "есть вопросы" }}</span>
        <span class="cnt">
          пунктов {{ props.review.checklist.length }} ·
          нарушено {{ props.review.checklist.filter((c) => !c.ok).length }}
        </span>
      </div>

      <div v-if="props.review.human_paragraph" class="par">{{ props.review.human_paragraph }}</div>

      <div v-if="props.review.checklist.length" class="checks">
        <div v-for="c in props.review.checklist" :key="c.item" class="chk" :class="{ bad: !c.ok }">
          <span class="mk">{{ c.ok ? "✓" : "✗" }}</span>
          <span class="nm">{{ itemWord(c.item) }}</span>
          <span v-if="c.note" class="nt">{{ c.note }}</span>
        </div>
      </div>

      <template v-if="props.review.questions.length">
        <div class="qh">что проверяющий просит уточнить · {{ props.review.questions.length }}</div>
        <ol class="qs">
          <li v-for="(q, i) in props.review.questions" :key="i">{{ q }}</li>
        </ol>
      </template>
    </template>
  </div>
</template>

<style scoped>
.sv{padding:2px 0}
.empty{color:var(--p-dim);font-size:12.5px;padding:4px 0}
.head{display:flex;align-items:baseline;gap:10px;margin-bottom:8px}
.head .vb{font-size:11px;letter-spacing:.09em;text-transform:uppercase;font-weight:600}
.head.ok .vb{color:var(--p-pass)}
.head.no .vb{color:#ffa972}
.head .cnt{font-size:10.5px;color:var(--p-dim2)}
.par{font-size:13px;line-height:1.55;color:#d7e0ea;margin-bottom:10px;white-space:pre-wrap;max-width:120ch}
.checks{display:flex;flex-direction:column;gap:4px;margin-bottom:10px}
.chk{display:flex;gap:9px;align-items:baseline;font-size:12.5px;color:#cfd9e4}
.chk .mk{flex:none;width:12px;color:var(--p-pass)}
.chk.bad .mk{color:var(--p-stop)}
.chk.bad .nm{color:#f2bcc2}
.chk .nm{flex:none;min-width:160px}
.chk .nt{color:var(--p-dim);font-size:11.5px}
.qh{font-size:10.5px;letter-spacing:.09em;text-transform:uppercase;color:#93a1b1;margin-bottom:5px}
.qs{margin:0;padding-left:18px;font-size:12.5px;line-height:1.55;color:#e2e9f1}
.qs li{margin-bottom:4px}
</style>
