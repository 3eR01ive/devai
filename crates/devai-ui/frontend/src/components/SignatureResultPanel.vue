<script setup lang="ts">
// Результат подписи (REQ-SIGN-06): самый новый сохранённый факт — кто, когда,
// каким обоснованием, к какой версии спеки. Источник — task.signature
// (решение или событие); rationale=null И пустая строка — один валидный факт
// «без обоснования» (R2 rework: Some("") достижим через CLI), поле не
// скрывается. Панель не сворачивается и не исчезает.
import { computed } from "vue";
import type { SignatureFact } from "../types";
import { fmtTime } from "../settings";
import { signatureRationale } from "../signature_view";
import { SIGNATURE_KIND_LABELS, SIGNATURE_SOURCE_LABELS, labelFor } from "../signature_labels";

const props = defineProps<{ fact: SignatureFact }>();

const rationale = computed(() => signatureRationale(props.fact.rationale));
</script>

<template>
  <div class="panel">
    <h3>Подпись спеки · результат</h3>
    <div class="body">
      <div class="rowflex">
        <b>{{ labelFor(SIGNATURE_KIND_LABELS, fact.kind).text }}</b>
        <span class="muted">{{ fact.actor }}</span>
        <span class="mono muted">{{ fmtTime(fact.at) }}</span>
        <span class="mono muted">{{ labelFor(SIGNATURE_SOURCE_LABELS, fact.source).text }}</span>
      </div>
      <p class="mono muted" style="margin: 2px 0 6px">
        версия спеки:
        <template v-if="fact.spec_seq !== null">{{ fact.spec_seq }}</template>
        <template v-else>версия в записи не сохранена</template>
      </p>
      <p class="muted" style="margin: 0 0 2px">обоснование:</p>
      <p v-if="rationale !== null" class="rationale">{{ rationale }}</p>
      <p v-else class="rationale muted">без обоснования</p>
    </div>
  </div>
</template>

<style scoped>
.rationale {
  margin: 0;
  white-space: pre-wrap;
}
</style>
