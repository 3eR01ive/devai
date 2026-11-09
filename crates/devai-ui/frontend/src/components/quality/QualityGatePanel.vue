<script setup lang="ts">
// Панель отчёта качества (REQ-QG-03/04/05/08/09): сохранённая форма
// раскрывается по пунктам, фильтруется по сохранённым группам, из находок
// собирается правимый черновик возврата, а действия гейта идут тем же путём,
// что CLI. Ничего не пересчитывается: группы, счётчики и строки — данные
// платформы; `report_view.ts`/`quality_gate_view.ts` держат чистые правила.
import { computed, ref } from "vue";
import {
  filterOptions,
  freshnessBanner,
  groupsForFilter,
  hasEvidence,
  isNameOnlyGroup,
  presentReport,
  showsPassedNote,
  siteText,
  type ReportBody,
} from "../../report_view";
import { addDraftItem, type GateAct, type ReturnDraftItem } from "../../quality_gate_view";
import type { HumanForm, HumanFormItem, QualityReportView, QualityGateView, UiError } from "../../types";
import { fmtTime, shortId } from "../../settings";
import StateChip from "../StateChip.vue";
import QualityGateActions from "./QualityGateActions.vue";

const props = defineProps<{
  report: QualityReportView | null;
  reportError: UiError | null;
  gate: QualityGateView | null;
  busy: boolean;
  error: UiError | null;
}>();

const emit = defineEmits<{
  (e: "apply", kind: GateAct, rationale: string): void;
}>();

const filter = ref("all");
const draft = ref<ReturnDraftItem[]>([]);

const body = computed<ReportBody | null>(() => (props.report ? presentReport(props.report) : null));
const form = computed<HumanForm | null>(() => (body.value?.kind === "form" ? body.value.form : null));
const banner = computed(() =>
  freshnessBanner(props.report?.freshness, props.report?.status ?? ""),
);
const shownGroups = computed(() => {
  const f = form.value;
  return f ? groupsForFilter(f, filter.value) : [];
});

function addPoint(it: HumanFormItem) {
  draft.value = addDraftItem(draft.value, it);
}
</script>

<template>
  <div>
    <!-- Отсутствие данных — честная пустота (REQ-SIGN-08): типизированный
         not_found значит «QA ещё не прогонялся», а не красную ошибку. -->
    <p v-if="props.reportError && props.reportError.code === 'not_found'" class="muted" style="margin: 0">
      QA ещё не прогонялся — отчёта качества по задаче нет.
    </p>
    <div v-else-if="props.reportError" class="errorbox" style="margin-bottom: 0">
      <span class="code">{{ props.reportError.code }}</span>{{ props.reportError.message }}
    </div>

    <template v-if="props.report">
      <div v-if="banner.stale" class="stale-banner">
        <strong>{{ banner.head }}</strong>
        <span class="mono muted">{{ banner.detail }}</span>
      </div>
      <div class="rowflex" style="margin-bottom: 6px">
        <span class="mono muted">artifact {{ shortId(props.report.artifact) }} · seq {{ props.report.seq }} · {{ props.report.contract }}</span>
        <span class="mono muted">создан {{ fmtTime(props.report.created_at) }}</span>
        <span class="mono muted">источник: {{ props.report.source }}</span>
        <StateChip :token="props.report.status" />
      </div>

      <!-- Сохранённая человеческая форма: группы в порядке отчёта, счётчики
           видны без раскрытия; фильтр только СКРЫВАЕТ строки, не пересобирая. -->
      <template v-if="body && body.kind === 'form' && form">
        <div class="qr-headline">
          <span class="mono">ВЕРДИКТ: {{ form.headline.verdict }} ({{ form.headline.machine_verdict }})</span>
          <span class="muted mono">{{ form.headline.numbers }}</span>
        </div>
        <p class="muted" style="margin: 2px 0 8px">{{ form.headline.basis }}</p>
        <div class="rowflex qr-counters">
          <p class="mono muted" style="margin: 0">
            бьёт по вердикту: {{ form.headline.counts.blocks }} ·
            нюансы: {{ form.headline.counts.nuances }} ·
            якоря: {{ form.headline.counts.anchors }} ·
            не проверено: {{ form.headline.counts.unchecked }}<template v-if="showsPassedNote(form)">
              · пройдено без замечаний: {{ form.headline.counts.passed }}</template>
          </p>
          <div class="spacer" />
          <label class="muted small">
            важность
            <select v-model="filter">
              <option v-for="o in filterOptions(form)" :key="o.key" :value="o.key">
                {{ o.title }} ({{ o.count }})
              </option>
            </select>
          </label>
        </div>

        <div v-for="g in shownGroups" :key="g.key" class="qr-group">
          <h4 class="qr-group-title">{{ g.title }} ({{ g.items.length }})</h4>
          <p v-if="g.lead" class="muted" style="margin: 0 0 4px">{{ g.lead }}</p>
          <ul class="qr-items">
            <li v-for="it in g.items" :key="it.id" class="qr-item-row">
              <template v-if="isNameOnlyGroup(g)">
                <span class="mono">{{ it.line }}</span>
              </template>
              <details v-else class="qr-item">
                <summary><span class="mono">{{ it.line }}</span></summary>
                <div class="qr-summary">{{ it.summary }}</div>
                <div class="muted qr-site">сайт: {{ siteText(it) }}</div>
                <div v-if="it.reason" class="muted qr-reason">причина: {{ it.reason }}</div>
                <details v-if="hasEvidence(it)" class="qr-evidence">
                  <summary class="muted">доказательства ({{ (it.evidence ?? []).length }})</summary>
                  <pre class="content">{{ (it.evidence ?? []).join("\n\n") }}</pre>
                </details>
              </details>
              <button class="add small" @click="addPoint(it)">в черновик возврата</button>
            </li>
          </ul>
        </div>
      </template>

      <template v-else-if="body && body.kind === 'damaged'">
        <div class="errorbox" style="margin-bottom: 8px">{{ body.marker }}</div>
        <pre v-if="body.text" class="content">{{ body.text }}</pre>
      </template>

      <pre v-else-if="body && (body.kind === 'flat' || body.kind === 'absent')" class="content">{{ body.text }}</pre>

      <details>
        <summary class="muted">machine-тело отчёта</summary>
        <pre class="content">{{ JSON.stringify(props.report.report, null, 2) }}</pre>
      </details>
    </template>

    <!-- Действия гейта — ровно тогда, когда платформа их публикует (QA). -->
    <QualityGateActions
      v-if="props.gate"
      :gate="props.gate"
      :draft="draft"
      :busy="props.busy"
      :error="props.error"
      @update:draft="draft = $event"
      @apply="(kind, rationale) => emit('apply', kind, rationale)"
    />
  </div>
</template>

<style scoped>
.qr-counters {
  margin-bottom: 10px;
}
.qr-item-row {
  display: flex;
  align-items: flex-start;
  gap: 8px;
}
.qr-item-row .qr-item {
  flex: 1;
}
.qr-item summary {
  cursor: pointer;
}
.qr-summary {
  margin-top: 4px;
}
.qr-site,
.qr-reason {
  font-size: 12px;
}
.add {
  flex: 0 0 auto;
}
.small {
  font-size: 12px;
}
</style>
