<script setup lang="ts">
// Панель графа ролей и этапов (REQ-RG-06). Рисует ГОТОВЫЙ объект
// `progress.graph`: порядок этапов, узлы, рёбра, позицию, петлю доработки,
// идущие прогоны, ожидания человека, число блокирующих вопросов и
// примечания. Окно не хранит порядка этапов, таблицы «состояние → узел»,
// словаря подписей и цветов статусов и ничего не пересчитывает — только
// раскладывает пришедшее. Пройдено/сейчас/дальше различаются чипом и словом
// без наведения; списки печатаются целиком (ни «+N ещё», ни прокрутки).
import { computed } from "vue";
import { fmtTime, shortId } from "../settings";
import type { GraphNode, RoleGraph } from "../types";

const props = defineProps<{ graph: RoleGraph; snapshotAt?: string }>();

const nodeDisplays = computed<Record<string, string>>(() => {
  const map: Record<string, string> = {};
  for (const n of props.graph.nodes) map[n.id] = n.display;
  return map;
});

function nodeName(id: string): string {
  return nodeDisplays.value[id] ?? id;
}

function nodeWait(node: GraphNode) {
  return props.graph.waits.find((w) => w.node === node.id) ?? null;
}

const position = computed(() => props.graph.position);
const rework = computed(() => props.graph.rework);
const blocking = computed(() => props.graph.blocking_questions);
</script>

<template>
  <div class="panel">
    <h3>Путь задачи · снимок {{ snapshotAt ? fmtTime(snapshotAt) : "—" }}</h3>
    <div class="body">
      <!-- Позиция: вид места словами платформы; off-graph — причина. -->
      <p style="margin: 0 0 6px">
        <b>{{ position.line }}</b>
        <span v-if="position.reason" class="mono muted"> · причина: {{ position.reason }}</span>
      </p>
      <div class="rowflex" style="margin-bottom: 10px">
        <span class="chip-metric">кругов доработки: {{ rework.rounds }}</span>
        <span v-if="rework.active" class="chip-status current">доработка идёт</span>
        <span class="chip-metric">открытых блокирующих вопросов: {{ blocking }}</span>
      </div>

      <!-- Ось этапов: порядок — массив платформы; статус — чип + слово. -->
      <div class="rowflex" style="margin-bottom: 12px">
        <template v-for="(s, i) in graph.stages" :key="s.state">
          <span class="chip-stage" :class="s.status">
            {{ s.label }}
            <span class="stage-word">· {{ s.status_word }}</span>
          </span>
          <span v-if="i < graph.stages.length - 1" class="muted">→</span>
        </template>
      </div>

      <!-- Идущие прогоны: имя роли и версия — те же слова, что у узла. -->
      <div v-if="graph.running.length" class="graph-section">
        <h4>кто исполняет сейчас</h4>
        <div v-for="r in graph.running" :key="r.execution_id" class="rowflex">
          <span class="mono">{{ r.role_display }}</span>
          <span class="muted">агент: {{ r.agent_name ?? "—" }}</span>
          <span v-if="r.note" class="chip-att">{{ r.note }}</span>
        </div>
      </div>

      <!-- Узлы в порядке этапов: все исполнения и артефакты целиком. -->
      <div v-for="n in graph.nodes" :key="n.id" class="node-card" :class="n.status">
        <div class="rowflex">
          <b>{{ n.display }}</b>
          <span class="chip-status" :class="n.status">{{ n.status_word }}</span>
          <span v-if="n.revoked" class="chip-att">результат отозван</span>
          <span v-if="n.kind === 'gate'" class="mono muted">гейт человека</span>
        </div>

        <div v-if="n.executions.length" class="node-block">
          <div class="mono muted">исполнения ({{ n.executions.length }})</div>
          <div v-for="e in n.executions" :key="e.id" class="rowflex">
            <span class="mono">{{ e.role_display }}</span>
            <span class="muted">агент: {{ e.agent_name ?? "—" }}</span>
            <span class="mono muted">{{ e.status }} · попытка {{ e.attempt }}</span>
            <span class="mono muted">{{ fmtTime(e.started_at) }} → {{ fmtTime(e.finished_at) }}</span>
          </div>
        </div>

        <div v-if="n.artifacts.length" class="node-block">
          <div class="mono muted">артефакты ({{ n.artifacts.length }})</div>
          <div v-for="a in n.artifacts" :key="a.id" class="rowflex">
            <span>{{ a.display }}</span>
            <span class="mono muted">{{ a.kind }} · {{ a.status }} · {{ shortId(a.id) }}</span>
          </div>
        </div>

        <!-- Ожидание человека: требование политики, подсказка, подпись и
             предъявление. Слова требования — данные объявления. -->
        <div v-if="n.kind === 'gate' && nodeWait(n)" class="node-block">
          <div>Требование: <b>{{ nodeWait(n)!.requirement }}</b></div>
          <div class="rowflex">
            <span v-if="nodeWait(n)!.awaiting" class="chip-status current">ждёт подписи</span>
            <span v-else class="muted">сейчас не ждёт</span>
            <span v-if="nodeWait(n)!.signable" class="chip-status passed">подписываемо</span>
            <span v-else class="muted">подписать нельзя</span>
          </div>
          <p class="mono" style="margin: 4px 0 0; white-space: pre-wrap">{{ nodeWait(n)!.hint }}</p>
          <div v-if="nodeWait(n)!.presentation" class="rowflex">
            <span>предъявлено: {{ nodeWait(n)!.presentation!.display }}</span>
            <span class="mono muted">{{ nodeWait(n)!.presentation!.status }}</span>
            <span v-if="nodeWait(n)!.presentation!.note" class="muted">
              {{ nodeWait(n)!.presentation!.note }}
            </span>
          </div>
        </div>
      </div>

      <!-- Петля доработки отдельным контуром, не точкой на оси. -->
      <div v-if="rework.rounds > 0" class="graph-section">
        <h4>контур доработки</h4>
        <p class="muted" style="margin: 0">{{ rework.note }}</p>
      </div>

      <!-- Рёбра: оба конца — существующие узлы, носитель назван. -->
      <div v-if="graph.edges.length" class="graph-section">
        <h4>переходы ({{ graph.edges.length }})</h4>
        <div v-for="(e, i) in graph.edges" :key="i" class="rowflex edge-line">
          <span class="mono">{{ nodeName(e.from) }} → {{ nodeName(e.to) }}</span>
          <span class="muted">{{ e.carrier.label }}</span>
          <span class="mono muted">{{ e.kind }}<template v-if="e.round !== null"> · круг {{ e.round }}</template></span>
        </div>
      </div>

      <div v-if="graph.notes.length" class="graph-section">
        <h4>примечания</h4>
        <p v-for="(note, i) in graph.notes" :key="i" class="muted" style="margin: 0 0 4px">
          {{ note }}
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.chip-stage {
  border: 1px solid var(--panel-border);
  border-radius: 4px;
  padding: 2px 8px;
  font-size: 12px;
}
.chip-stage.passed {
  color: var(--green, #57a64a);
  border-color: var(--green, #57a64a);
}
.chip-stage.current {
  color: var(--orange);
  border-color: var(--orange);
  font-weight: 700;
}
.chip-stage.pending {
  color: var(--muted);
}
.stage-word {
  font-weight: 400;
  opacity: 0.85;
}
.chip-status {
  border-radius: 4px;
  padding: 1px 8px;
  font-size: 12px;
  border: 1px solid var(--panel-border);
}
.chip-status.passed {
  color: var(--green, #57a64a);
  border-color: var(--green, #57a64a);
}
.chip-status.current {
  color: var(--orange);
  border-color: var(--orange);
  font-weight: 700;
}
.chip-status.expected {
  color: var(--muted);
}
.chip-metric {
  border: 1px solid var(--panel-border);
  border-radius: 4px;
  padding: 1px 8px;
  font-size: 12px;
}
.chip-att {
  color: var(--orange);
  font-weight: 700;
  font-size: 12px;
}
.node-card {
  border-left: 3px solid var(--panel-border);
  padding: 6px 0 6px 10px;
  margin-bottom: 8px;
}
.node-card.current {
  border-left-color: var(--orange);
}
.node-card.passed {
  border-left-color: var(--green, #57a64a);
}
.node-block {
  margin-top: 6px;
}
.graph-section {
  margin-top: 12px;
}
.graph-section h4 {
  margin: 0 0 4px;
  font-size: 13px;
  font-weight: 600;
}
.edge-line {
  flex-wrap: wrap;
}
</style>
