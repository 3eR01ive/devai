<script setup lang="ts">
// Панель отчёта качества (REQ-QG-03/04/05/08/09, REQ-SAY-01..06): экран
// говорит по-человечески и решает человек. Сверху — работа и то, что от него
// хотят; две кнопки подписаны последствием; оценка аудита — словами, рядом
// сохранённый балл работы (решение оператора 10.11); каждое замечание — фразой
// (что не так, где, чем грозит), а служебное имя проверки и остальные числа
// уходят в раскрытие; «не проверено» стоит отдельным разделом, не вперемешку с
// недостатками; черновик возврата приходит заполненным. Устаревший отчёт первым
// делом объясняет обычными словами, что решать по нему нельзя и что делать.
// Ничего не пересчитывается: первый слой приходит готовым от платформы
// (`quality_gate_view.ts` держит чистые правила окна), а подробности —
// сохранённые группы отчёта.
import { computed, ref, watch } from "vue";
import {
  filterOptions,
  freshnessBanner,
  groupsForFilter,
  hasEvidence,
  presentReport,
  showsPassedNote,
  siteText,
  type ReportBody,
} from "../../report_view";
import {
  decisionPrompt,
  inDraft,
  prefillDraft,
  toggleDraftItem,
  type GateAct,
  type ReturnDraftItem,
} from "../../quality_gate_view";
import type {
  FirstLayer,
  FirstLayerRow,
  HumanForm,
  HumanFormItem,
  QualityReportView,
  QualityGateView,
  UiError,
} from "../../types";
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
// Артефакт, под который черновик уже собран: смена отчёта заполняет заново,
// «снять все» человеком больше не воскрешается.
const seededFor = ref<string | null>(null);

const body = computed<ReportBody | null>(() => (props.report ? presentReport(props.report) : null));
const form = computed<HumanForm | null>(() => (body.value?.kind === "form" ? body.value.form : null));
/** Первый слой платформы: только у читаемой человеческой формы. */
const firstLayer = computed<FirstLayer | null>(() =>
  body.value && body.value.kind === "form" ? (props.report?.first_layer ?? null) : null,
);
const prompt = computed(() => decisionPrompt(props.gate));
const banner = computed(() =>
  freshnessBanner(props.report?.freshness, props.report?.status ?? ""),
);
const shownGroups = computed(() => {
  const f = form.value;
  return f ? groupsForFilter(f, filter.value) : [];
});
const draftNote = computed(() => {
  const fl = firstLayer.value;
  if (draft.value.length) {
    return "все замечания ниже уже здесь — сними те, что не надо возвращать.";
  }
  if (fl && fl.findings.length) {
    return "пунктов нет — возврат пойдёт с одним обоснованием.";
  }
  return "возвращать нечего: недостатков в отчёте нет.";
});

watch(
  () => [props.report?.artifact ?? "", firstLayer.value] as const,
  () => {
    const key = props.report?.artifact ?? "";
    const fl = firstLayer.value;
    if (!key || !fl) return;
    if (seededFor.value === key) return;
    draft.value = prefillDraft(fl.findings);
    seededFor.value = key;
  },
  { immediate: true },
);

function toggle(it: FirstLayerRow) {
  draft.value = toggleDraftItem(draft.value, { id: it.id, summary: it.what, site: it.site });
}

function addPoint(it: HumanFormItem) {
  draft.value = toggleDraftItem(draft.value, { id: it.id, summary: it.summary, site: it.site });
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
      <!-- ПЕРВЫЙ СЛОЙ: что от человека хотят и что нашёл аудит — словами.
           Устаревший отчёт первым делом объясняет обычными словами, что
           решать по нему нельзя и что делать, — и не призывает принять или
           вернуть (REQ-SAY-01 AC7). -->
      <div class="qr-head">
        <p v-if="banner.stale" class="qr-stale">{{ banner.plain }}</p>
        <template v-else>
          <p class="qr-appeal">{{ prompt.head }}</p>
          <p v-if="prompt.why" class="muted qr-why">{{ prompt.why }}</p>
        </template>
        <p v-if="firstLayer" class="qr-found">{{ firstLayer.found }}</p>
        <p v-if="firstLayer" class="muted qr-score">{{ firstLayer.score_line }}</p>
      </div>

      <!-- Действия гейта — ровно тогда, когда платформа их публикует (QA) и
           отчёт по-прежнему текущий: по устаревшему отчёту решение не
           предлагается. -->
      <QualityGateActions
        v-if="props.gate && !banner.stale"
        :gate="props.gate"
        :draft="draft"
        :busy="props.busy"
        :error="props.error"
        :draft-note="draftNote"
        :unchecked="firstLayer?.unchecked ?? []"
        @update:draft="draft = $event"
        @apply="(kind, rationale) => emit('apply', kind, rationale)"
      />

      <!-- ЗАМЕЧАНИЯ: человеческая фраза первой; что не так, где и чем грозит.
           Идентификатор и числа — в раскрытии пункта. -->
      <section v-if="firstLayer && firstLayer.findings.length" class="qr-findings">
        <h4 class="qr-section">Что нашёл аудит</h4>
        <ul class="qr-items">
          <li v-for="r in firstLayer.findings" :key="r.id" class="qr-item-row">
            <details class="qr-item">
              <summary>
                <span class="qr-what">{{ r.what }}</span>
                <span class="muted qr-where">где: {{ siteText(r.site) }}</span>
                <span class="muted qr-stake">{{ r.note }}</span>
              </summary>
              <p class="muted qr-machine">машинная строка: <span class="mono">{{ r.line }}</span></p>
              <details v-if="r.evidence && r.evidence.length" class="qr-evidence">
                <summary class="muted">доказательства ({{ r.evidence.length }})</summary>
                <pre class="content">{{ r.evidence.join("\n\n") }}</pre>
              </details>
            </details>
            <button class="add small" @click="toggle(r)">
              {{ inDraft(draft, r.id) ? "снять из черновика" : "в черновик возврата" }}
            </button>
          </li>
        </ul>
      </section>

      <!-- НЕ ПРОВЕРЕНО: отдельный раздел, это нехватка данных, а не дефект. -->
      <section v-if="firstLayer && firstLayer.unchecked.length" class="qr-unchecked">
        <h4 class="qr-section">Не проверено — чего не хватило ({{ firstLayer.unchecked.length }})</h4>
        <p class="muted qr-lead">{{ firstLayer.unchecked_lead }}</p>
        <ul class="qr-items">
          <li v-for="r in firstLayer.unchecked" :key="r.id" class="qr-item-row">
            <details class="qr-item">
              <summary><span class="qr-what">{{ r.what }}</span></summary>
              <p class="muted qr-where">где: {{ siteText(r.site) }}</p>
              <p class="muted">чего не хватило: {{ r.missing }}</p>
              <p class="muted">что делать: {{ r.note }}</p>
              <p class="muted qr-machine">машинная строка: <span class="mono">{{ r.line }}</span></p>
              <details v-if="r.evidence && r.evidence.length" class="qr-evidence">
                <summary class="muted">доказательства ({{ r.evidence.length }})</summary>
                <pre class="content">{{ r.evidence.join("\n\n") }}</pre>
              </details>
            </details>
          </li>
        </ul>
      </section>

      <!-- Старое тело без человеческой формы: показываем честно, как есть. -->
      <template v-if="!firstLayer">
        <p class="muted small" style="margin: 8px 0 0">
          человеческой подачи в этом отчёте нет — ниже сохранённый вид без изменений
        </p>
        <template v-if="body && body.kind === 'damaged'">
          <div class="errorbox" style="margin-bottom: 8px">{{ body.marker }}</div>
          <pre v-if="body.text" class="content">{{ body.text }}</pre>
        </template>
        <pre v-else-if="body && (body.kind === 'flat' || body.kind === 'absent')" class="content">{{ body.text }}</pre>
      </template>

      <!-- ПОДРОБНОСТИ И ЧИСЛА: всё, что было на экране раньше, — никуда не
           делось, только больше не первым (REQ-SAY-06 AC1/AC2). -->
      <details class="qr-details" :open="!firstLayer">
        <summary class="muted">подробности и числа</summary>
        <!-- Устаревание: платформенный маркер и сырые факты стора — здесь, а
             не в первом слое (REQ-SAY-01 AC7). -->
        <div v-if="banner.stale" class="qr-stale-facts">
          <p class="mono muted" style="margin: 6px 0 0">{{ banner.head }}</p>
          <p class="mono muted" style="margin: 2px 0 0">{{ banner.detail }}</p>
        </div>
        <div class="rowflex" style="margin: 6px 0">
          <span class="mono muted">artifact {{ shortId(props.report.artifact) }} · seq {{ props.report.seq }} · {{ props.report.contract }}</span>
          <span class="mono muted">создан {{ fmtTime(props.report.created_at) }}</span>
          <span class="mono muted">источник: {{ props.report.source }}</span>
          <StateChip :token="props.report.status" />
        </div>

        <template v-if="form">
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
                <!-- Закреплённое читается как замечание (REQ-SAY-03 AC6):
                     человеческая фраза первой, служебная строка — в раскрытии. -->
                <details v-if="g.key === 'anchors'" class="qr-item">
                  <summary><span class="qr-what">{{ it.summary }}</span></summary>
                  <p class="muted qr-machine">машинная строка: <span class="mono">{{ it.line }}</span></p>
                </details>
                <details v-else class="qr-item">
                  <summary><span class="mono">{{ it.line }}</span></summary>
                  <div class="qr-summary">{{ it.summary }}</div>
                  <div class="muted qr-site">где: {{ siteText(it.site) }}</div>
                  <div v-if="it.reason" class="muted qr-reason">причина: {{ it.reason }}</div>
                  <details v-if="hasEvidence(it)" class="qr-evidence">
                    <summary class="muted">доказательства ({{ (it.evidence ?? []).length }})</summary>
                    <pre class="content">{{ (it.evidence ?? []).join("\n\n") }}</pre>
                  </details>
                </details>
                <button v-if="g.key !== 'anchors'" class="add small" @click="addPoint(it)">
                  {{ inDraft(draft, it.id) ? "снять из черновика" : "в черновик возврата" }}
                </button>
              </li>
            </ul>
          </div>
        </template>

        <details>
          <summary class="muted">machine-тело отчёта</summary>
          <pre class="content">{{ JSON.stringify(props.report.report, null, 2) }}</pre>
        </details>
      </details>
    </template>
  </div>
</template>

<style scoped>
.qr-head {
  margin: 4px 0 6px;
}
.qr-appeal {
  margin: 0;
  font-weight: 600;
}
.qr-stale {
  margin: 0;
  font-weight: 600;
}
.qr-score {
  margin: 2px 0 0;
}
.qr-why {
  margin: 2px 0 0;
}
.qr-found {
  margin: 6px 0 0;
}
.qr-section {
  margin: 12px 0 4px;
}
.qr-lead {
  margin: 0 0 4px;
}
.qr-what {
  font-weight: 600;
}
.qr-where {
  margin-left: 8px;
  font-size: 12px;
}
.qr-stake {
  margin-left: 8px;
  font-size: 12px;
}
.qr-machine {
  font-size: 12px;
  margin: 4px 0 0;
}
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
.qr-details {
  margin-top: 12px;
}
.add {
  flex: 0 0 auto;
}
.small {
  font-size: 12px;
}
</style>
