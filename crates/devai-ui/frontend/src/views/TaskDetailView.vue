<script setup lang="ts">
// Детали задачи (REQ-UI-05…REQ-UI-08, REQ-UI-10, REQ-UI-11): состояние,
// пайплайн, артефакты, отчёт качества, решения/события, зависимости,
// исполнения и воркспейс — каждое поле из готовой typed-view платформы.
// UI не хранит свою очерёдность стадий, не переприсваивает статусы
// артефактов и не выдумывает строки воркспейсов.
import { computed, onMounted, ref } from "vue";
import { api, toUiError } from "../api";
import { fmtElapsed, fmtTime, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import {
  freshnessBanner,
  hasEvidence,
  isNameOnlyGroup,
  presentReport,
  showsPassedNote,
  visibleGroups,
  type ReportBody,
} from "../report_view";
import type {
  ActiveRow,
  Artifact,
  ArtifactContent,
  Decision,
  DependenciesView,
  LivenessView,
  ProgressView,
  QualityReportView,
  RoleExecution,
  StatusView,
  UiError,
  WorkflowEvent,
  WorkspaceChangesView,
  WorkspaceDiffView,
  WorkspaceFileChange,
} from "../types";

const props = defineProps<{ id: string }>();

const status = ref<StatusView | null>(null);
const progress = ref<ProgressView | null>(null);
const deps = ref<DependenciesView | null>(null);
const artifacts = ref<Artifact[]>([]);
const decisions = ref<Decision[]>([]);
const events = ref<WorkflowEvent[]>([]);
const executions = ref<RoleExecution[]>([]);
const activeRows = ref<ActiveRow[]>([]);
const liveness = ref<Record<string, LivenessView>>({});
const report = ref<QualityReportView | null>(null);
const reportError = ref<UiError | null>(null);
const error = ref<UiError | null>(null);
const content = ref<ArtifactContent | null>(null);
const contentFor = ref<string | null>(null);
const eventsLimit = ref(50);
// Воркспейс-панель (REQ-WV-04/05): changeset из task.workspace_changes,
// дифф выбранного файла из task.workspace_diff — обе операции read-плоскости.
const ws = ref<WorkspaceChangesView | null>(null);
const wsError = ref<UiError | null>(null);
const wsDiff = ref<WorkspaceDiffView | null>(null);
const wsDiffFor = ref<string | null>(null);
const wsDiffError = ref<UiError | null>(null);

async function loadAll() {
  error.value = null;
  try {
    const [s, p, d, arts, decs, execs, snap] = await Promise.all([
      api.taskStatus(props.id),
      api.taskProgress(props.id),
      api.taskDependencies(props.id),
      api.artifactsList(props.id),
      api.taskDecisions(props.id),
      api.executionsList(props.id),
      api.taskActive(),
    ]);
    status.value = s;
    progress.value = p;
    deps.value = d;
    artifacts.value = arts;
    decisions.value = decs;
    executions.value = execs;
    activeRows.value = snap.rows.filter((r) => r.task_id === props.id || r.task_id.startsWith(props.id));
    // Liveness — наблюдение (REQ-SW-06): только read exec.liveness по RUNNING-строкам.
    for (const e of execs.filter((x) => x.status === "RUNNING")) {
      try {
        liveness.value[e.id] = await api.executionLiveness(e.id);
      } catch (err) {
        liveness.value[e.id] = {
          id: e.id, task_id: e.task_id, status: e.status, owner_pid: null,
          owner_host: null, owner_process_start: null, liveness: "unknown",
        } satisfies LivenessView;
        void err;
      }
    }
    await loadEvents();
    await loadReport();
    await loadWorkspace();
  } catch (e) {
    error.value = toUiError(e);
  }
}

// Changeset задачи (task.workspace_changes). Ошибкой loadAll это не
// становится: панель честно показывает собственный вердикт/ошибку.
async function loadWorkspace() {
  wsError.value = null;
  wsDiff.value = null;
  wsDiffFor.value = null;
  wsDiffError.value = null;
  try {
    ws.value = await api.taskWorkspaceChanges(props.id);
  } catch (e) {
    ws.value = null;
    wsError.value = toUiError(e);
  }
}

// Дифф одного выбранного файла (REQ-WV-05 AC1: выбор меняет только блок
// диффа); файл вне листа — платформенный not_found по коду (AC2 REQ-WV-02).
async function openWorkspaceDiff(f: WorkspaceFileChange) {
  wsDiffFor.value = f.path;
  wsDiff.value = null;
  wsDiffError.value = null;
  try {
    wsDiff.value = await api.taskWorkspaceDiff(props.id, f.path);
  } catch (e) {
    wsDiffError.value = toUiError(e);
  }
}

async function loadEvents() {
  try {
    events.value = await api.taskEvents(props.id, eventsLimit.value);
  } catch (e) {
    error.value = toUiError(e);
  }
}

async function loadReport() {
  report.value = null;
  reportError.value = null;
  try {
    report.value = await api.taskReport(props.id);
  } catch (e) {
    reportError.value = toUiError(e); // NotFound «QA has not run» — platform text as-is (REQ-UI-07 AC2)
  }
}

// Человеческая форма отчёта — сохранённый при публикации `human_view`
// (REQ-QH-01): рендерим его поля как есть, порядок групп, баллы и важность
// не пересчитываются в UI. Какую ветку показать (форма / повреждённая форма /
// плоский текст / пусто) решает `report_view.ts` — по source API и с
// проверкой формы, чтобы структурно битый blob не ронял панель (rework R2;
// ветки покрыты tests/report_view.test.ts, R3).
const reportBody = computed<ReportBody | null>(() =>
  report.value ? presentReport(report.value) : null,
);

// Баннер свежести (REQ-QH-05, rework R5): шапка — маркер платформы как есть,
// строка факта — статус отчёта, фаза задачи и исполнение. Ветка живёт в
// `report_view.ts` и покрыта тестом, шаблон только раскладывает готовое.
const reportBanner = computed(() =>
  freshnessBanner(report.value?.freshness, report.value?.status ?? ""),
);

async function openContent(a: Artifact) {
  contentFor.value = a.id;
  content.value = null;
  try {
    content.value = await api.artifactContent(a.id);
  } catch (e) {
    error.value = toUiError(e);
  }
}

onMounted(loadAll);
</script>

<template>
  <section v-if="error" class="errorbox">
    <span class="code">{{ error.code }}</span>{{ error.message }}
  </section>

  <section v-if="status">
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">{{ status.task.title }}</h2>
      <StateChip :token="status.task.state" />
      <StateChip v-if="status.task.return_state" :token="status.task.return_state" />
      <div class="spacer" />
      <span class="mono muted">{{ status.task.id }}</span>
      <button @click="loadAll">Обновить</button>
    </div>
    <p class="muted" style="white-space: pre-wrap">{{ status.task.business_description }}</p>

    <!-- Пайплайн: порядок отдаёт платформа (PipelineView), UI не хранит свой -->
    <div v-if="progress" class="panel">
      <h3>Пайплайн · снимок {{ fmtTime(progress.snapshot_at) }}</h3>
      <div class="body">
        <div class="rowflex">
          <template v-for="(stage, i) in progress.pipeline.order" :key="stage">
            <span
              class="chip-stage"
              :class="{ here: progress.pipeline.position_index === i }"
            >{{ stage }}</span>
            <span v-if="i < progress.pipeline.order.length - 1" class="muted">→</span>
          </template>
        </div>
        <p v-if="progress.pipeline.off_spine || progress.pipeline.position === null" class="muted" style="margin: 8px 0 0">
          вне линейки стадий; текущее состояние — <b class="mono">{{ progress.pipeline.current }}</b>
          <template v-if="progress.pipeline.terminal"> (терминальное)</template>
        </p>
      </div>
    </div>

    <div class="panel">
      <h3>Разрешённые операции (данные workflow::allowed_ops)</h3>
      <div class="body rowflex">
        <span v-for="op in status.allowed_operations" :key="op" class="mono chip-op">{{ op }}</span>
        <span v-if="!status.allowed_operations.length" class="muted">—</span>
      </div>
    </div>

    <div v-if="status.open_questions.length" class="panel">
      <h3>Открытые вопросы</h3>
      <div class="body">
        <div v-for="q in status.open_questions" :key="q.id" style="margin-bottom: 10px">
          <div class="rowflex">
            <StateChip token="OPEN" />
            <span v-if="q.blocking" class="chip-att">● блокирует линию</span>
          </div>
          <p style="margin: 4px 0">{{ q.text }}</p>
          <p v-if="q.context" class="muted" style="margin: 0 0 4px; white-space: pre-wrap">{{ q.context }}</p>
          <div v-if="q.options.length" class="rowflex">
            <span v-for="o in q.options" :key="o" class="mono chip-op">{{ o }}</span>
          </div>
        </div>
        <p class="muted">Ответ на вопрос — из CLI (<span class="mono">devai question answer</span>); GUI этой итерации наблюдает.</p>
      </div>
    </div>

    <!-- Воркспейс: branch/worktree только там, где их опубликовала платформа
         (ActiveRow для RUNNING + событие worktree.prepared) — REQ-UI-11.
         Changeset против baseline — read-операции task.workspace_changes /
         task.workspace_diff (REQ-WV-04/05): UI не знает ни о git, ни о
         путях — вердикты состояний приходят как данные. -->
    <div class="panel">
      <h3>Воркспейс (worktree задачи · изменения против baseline)</h3>
      <div class="body">
        <template v-if="activeRows.length">
          <div v-for="r in activeRows" :key="r.id" class="rowflex" style="margin-bottom: 6px">
            <StateChip :token="r.state" />
            <span class="mono">{{ r.role }}</span>
            <span v-if="r.branch" class="mono muted">branch {{ r.branch }}</span>
            <span v-if="r.worktree" class="mono muted">{{ r.worktree }}</span>
          </div>
        </template>

        <div v-if="wsError" class="errorbox" style="margin-bottom: 0">
          <span class="code">{{ wsError.code }}</span>{{ wsError.message }}
        </div>

        <template v-else-if="ws">
          <p v-if="ws.state === 'no_isolation'" class="muted" style="margin: 0">
            Изоляции не было — у задачи нет события
            <span class="mono">worktree.prepared</span>; changeset показывать
            нечего (вердикт платформы <span class="mono">no_isolation</span>).
          </p>
          <template v-else>
            <div class="rowflex" style="margin-bottom: 6px">
              <StateChip :token="ws.state" />
              <span v-if="ws.branch" class="mono muted">branch {{ ws.branch }}</span>
              <span v-if="ws.bound_head" class="mono muted">baseline {{ shortId(ws.bound_head) }}</span>
              <span v-if="ws.tip" class="mono muted">tip {{ shortId(ws.tip) }}</span>
            </div>
            <p v-if="ws.note" class="muted" style="margin: 0 0 6px">{{ ws.note }}</p>
            <p v-if="ws.zero_changes" style="margin: 0 0 6px">
              Изоляция состоялась, поверх baseline коммитов нет —
              <b>ноль изменений</b> (легальный результат, не недоступность).
            </p>
            <div v-if="ws.commits.length" style="margin-bottom: 6px">
              <div v-for="c in ws.commits" :key="c.commit + c.execution" class="rowflex">
                <span class="mono muted">прогон {{ shortId(c.execution) }}</span>
                <span class="mono muted">commit {{ shortId(c.commit) }}</span>
                <span class="mono muted">{{ fmtTime(c.at) }}</span>
              </div>
            </div>
            <p v-if="ws.head_before && ws.head_after" class="muted" style="margin: 0 0 6px">
              смержено в main: <span class="mono">{{ shortId(ws.head_before) }}</span> →
              <span class="mono">{{ shortId(ws.head_after) }}</span>
            </p>

            <table v-if="ws.files.length" class="data">
              <thead>
                <tr><th>Что</th><th>Путь</th><th></th></tr>
              </thead>
              <tbody>
                <tr v-for="f in ws.files" :key="f.path">
                  <td class="mono">{{ f.status }}</td>
                  <td class="mono">{{ f.path }}</td>
                  <td><button @click="openWorkspaceDiff(f)">дифф</button></td>
                </tr>
              </tbody>
            </table>

            <div v-if="wsDiffFor">
              <p class="mono muted" style="margin: 8px 0 0">
                diff: {{ wsDiffFor }}
                <template v-if="wsDiff && wsDiff.baseline && wsDiff.tip">
                  · {{ shortId(wsDiff.baseline) }}..{{ shortId(wsDiff.tip) }}
                </template>
              </p>
              <div v-if="wsDiffError" class="errorbox" style="margin-bottom: 0">
                <span class="code">{{ wsDiffError.code }}</span>{{ wsDiffError.message }}
              </div>
              <pre v-else-if="wsDiff && wsDiff.diff !== null" class="content">{{ wsDiff.diff }}</pre>
              <p v-else-if="wsDiff && wsDiff.binary" class="muted" style="margin: 4px 0 0">
                Изменён, бинарный<template v-if="wsDiff.size_bytes !== null"> ({{ wsDiff.size_bytes }} байт)</template> — сырые байты не рендерим.
              </p>
            </div>
          </template>
        </template>
        <p v-else class="muted" style="margin: 0">Загрузка changeset…</p>
      </div>
    </div>

    <div v-if="deps && (deps.outgoing.length || deps.incoming.length)" class="panel">
      <h3>Зависимости</h3>
      <div class="body">
        <div v-for="d in deps.outgoing" :key="d.id" class="rowflex" style="margin-bottom: 4px">
          <span class="mono muted">→ {{ d.kind }}</span>
          <RouterLink class="title-link" :to="`/task/${d.depends_on_id}`">{{ d.depends_on_title }}</RouterLink>
          <StateChip :token="d.depends_on_state" />
        </div>
        <div v-for="d in deps.incoming" :key="d.id" class="rowflex" style="margin-bottom: 4px">
          <span class="mono muted">← {{ d.kind }}</span>
          <RouterLink class="title-link" :to="`/task/${d.task_id}`">{{ d.task_title }}</RouterLink>
          <StateChip :token="d.task_state" />
        </div>
      </div>
    </div>

    <div class="panel">
      <h3>Артефакты (kind · seq · статус — статусы хранимые)</h3>
      <div class="body">
        <table class="data">
          <thead>
            <tr><th>Вид</th><th>Имя</th><th>Seq</th><th>Статус</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="a in artifacts" :key="a.id">
              <td class="mono">{{ a.kind }}</td>
              <td>{{ a.name }} <span class="muted mono">{{ shortId(a.id) }}</span></td>
              <td>{{ a.seq }}</td>
              <td><StateChip :token="a.status" /></td>
              <td><button @click="openContent(a)">содержимое</button></td>
            </tr>
          </tbody>
        </table>
        <div v-if="contentFor">
          <p class="mono muted" style="margin: 8px 0 0">artifact: {{ shortId(contentFor) }}</p>
          <pre v-if="content && content.shape === 'text'" class="content">{{ content.text }}</pre>
          <p v-else-if="content && content.shape === 'binary'" class="muted">
            Бинарное содержимое ({{ content.size }} байт) — платформа ответила NotUtf8, не рендерим.
          </p>
        </div>
      </div>
    </div>

    <div class="panel">
      <h3>Отчёт качества (task.report)</h3>
      <div class="body">
        <div v-if="reportError" class="errorbox" style="margin-bottom: 0">
          <span class="code">{{ reportError.code }}</span>{{ reportError.message }}
        </div>
        <template v-else-if="report">
          <div v-if="reportBanner.stale" class="stale-banner">
            <strong>{{ reportBanner.head }}</strong>
            <span class="mono muted">{{ reportBanner.detail }}</span>
          </div>
          <div class="rowflex" style="margin-bottom: 6px">
            <span class="mono muted">artifact {{ shortId(report.artifact) }} · seq {{ report.seq }} · {{ report.contract }}</span>
            <span class="mono muted">создан {{ fmtTime(report.created_at) }}</span>
            <!-- Что именно отрендерено (rework R4): форма, плоский текст
                 или маркер повреждения — не угадывается по тексту. -->
            <span class="mono muted">источник: {{ report.source }}</span>
            <StateChip :token="report.status" />
          </div>
          <!-- Человеческая форма (REQ-QH-03/04): группы читаются из
               сохранённого human_view как есть — UI ничего не группирует и не
               пересчитывает; код и доказательства живут в свёртке пункта.
               Ветку выбирает report_view.ts (rework R2/R3): битая форма даёт
               честный маркер, а не падение панели. -->
          <template v-if="reportBody && reportBody.kind === 'form'">
            <div class="qr-headline">
              <span class="mono">ВЕРДИКТ: {{ reportBody.form.headline.verdict }} ({{ reportBody.form.headline.machine_verdict }})</span>
              <span class="muted mono">{{ reportBody.form.headline.numbers }}</span>
            </div>
            <p class="muted" style="margin: 2px 0 8px">{{ reportBody.form.headline.basis }}</p>
            <p class="mono muted" style="margin: 0 0 10px">
              бьёт по вердикту: {{ reportBody.form.headline.counts.blocks }} ·
              нюансы: {{ reportBody.form.headline.counts.nuances }} ·
              якоря: {{ reportBody.form.headline.counts.anchors }} ·
              не проверено: {{ reportBody.form.headline.counts.unchecked }}<template
                v-if="showsPassedNote(reportBody.form)"
              > · пройдено без замечаний: {{ reportBody.form.headline.counts.passed }}</template>
            </p>
            <div v-for="g in visibleGroups(reportBody.form)" :key="g.key" class="qr-group">
              <h4 class="qr-group-title">{{ g.title }} ({{ g.items.length }})</h4>
              <p v-if="g.lead" class="muted" style="margin: 0 0 4px">{{ g.lead }}</p>
              <ul class="qr-items">
                <li v-for="it in g.items" :key="it.id">
                  <template v-if="isNameOnlyGroup(g)">
                    <span class="mono">{{ it.line }}</span>
                  </template>
                  <template v-else>
                    <div class="qr-item-line">
                      <span class="mono">{{ it.line }}</span>
                    </div>
                    <div class="qr-summary">{{ it.summary }}</div>
                    <div v-if="it.reason" class="muted qr-reason">причина: {{ it.reason }}</div>
                    <details v-if="hasEvidence(it)" class="qr-evidence">
                      <summary class="muted">доказательства ({{ (it.evidence ?? []).length }})</summary>
                      <pre class="content">{{ (it.evidence ?? []).join("\n\n") }}</pre>
                    </details>
                  </template>
                </li>
              </ul>
            </div>
          </template>
          <template v-else-if="reportBody && reportBody.kind === 'damaged'">
            <div class="errorbox" style="margin-bottom: 8px">{{ reportBody.marker }}</div>
            <pre v-if="reportBody.text" class="content">{{ reportBody.text }}</pre>
          </template>
          <pre
            v-else-if="reportBody && (reportBody.kind === 'flat' || reportBody.kind === 'absent')"
            class="content"
          >{{ reportBody.text }}</pre>
          <details>
            <summary class="muted">machine-тело отчёта</summary>
            <pre class="content">{{ JSON.stringify(report.report, null, 2) }}</pre>
          </details>
        </template>
      </div>
    </div>

    <div class="panel">
      <h3>Исполнения ролей (exec.list; живые — exec.liveness)</h3>
      <div class="body">
        <table class="data">
          <thead>
            <tr><th>Роль@вер</th><th>Агент</th><th>Статус</th><th>Live</th><th>Старт / финиш</th><th>Класс сбоя</th></tr>
          </thead>
          <tbody>
            <tr v-for="e in executions" :key="e.id">
              <td class="mono">{{ e.role_slug }}@{{ e.role_version }} <span class="muted">·{{ e.attempt ?? 1 }}</span></td>
              <td>{{ e.agent_name ?? "—" }}</td>
              <td><StateChip :token="e.status" /></td>
              <td>
                <StateChip v-if="liveness[e.id]" :token="liveness[e.id].liveness" />
                <span v-else class="muted">—</span>
              </td>
              <td class="mono muted">{{ fmtTime(e.started_at) }} → {{ fmtTime(e.finished_at) }}</td>
              <td class="mono muted">{{ e.failure_class ?? "—" }}</td>
            </tr>
          </tbody>
        </table>
        <p v-if="progress && progress.running_executions.length" class="muted" style="margin: 8px 0 0">
          <span v-for="r in progress.running_executions" :key="r.id" style="margin-right: 14px">
            {{ r.role }} · {{ r.agent_name ?? "?" }} · {{ fmtElapsed(r.elapsed_seconds) }}
          </span>
        </p>
      </div>
    </div>

    <div class="panel">
      <h3>Решения (task.decisions)</h3>
      <div class="body">
        <div v-for="d in decisions" :key="d.id" style="margin-bottom: 8px">
          <div class="rowflex">
            <span class="mono">{{ d.kind }}</span>
            <StateChip :token="d.verdict" />
            <span class="muted">{{ d.actor }} · {{ fmtTime(d.created_at) }}</span>
          </div>
          <details v-if="d.rationale">
            <summary class="muted">rationale</summary>
            <pre class="content">{{ d.rationale }}</pre>
          </details>
        </div>
        <p v-if="!decisions.length" class="muted" style="margin: 0">Решений пока нет.</p>
      </div>
    </div>

    <div class="panel">
      <h3>События (task.events, newest tail) · limit</h3>
      <div class="body">
        <div class="rowflex" style="margin-bottom: 8px">
          <input v-model.number="eventsLimit" type="number" min="1" max="500" style="width: 80px" />
          <button @click="loadEvents">применить</button>
        </div>
        <table class="data">
          <tbody>
            <tr v-for="ev in events" :key="ev.id">
              <td class="mono">{{ ev.kind }}</td>
              <td class="muted">{{ ev.actor }}</td>
              <td class="mono muted">{{ fmtTime(ev.created_at) }}</td>
              <td>
                <details v-if="ev.payload && JSON.stringify(ev.payload) !== '{}'">
                  <summary class="muted mono">payload</summary>
                  <pre class="content">{{ JSON.stringify(ev.payload, null, 2) }}</pre>
                </details>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-if="!events.length" class="muted" style="margin: 0">Событий нет.</p>
      </div>
    </div>
  </section>
  <section v-else-if="!error" class="empty">Загрузка…</section>
</template>

<style scoped>
.chip-stage {
  padding: 2px 8px;
  border: 1px solid var(--slate);
  border-radius: 4px;
  font-family: var(--mono);
  font-size: 12px;
  color: var(--slate);
}
.chip-stage.here {
  border-color: var(--orange);
  background: var(--orange);
  color: var(--coal-lo);
  font-weight: 700;
}
.chip-op {
  padding: 1px 8px;
  border: 1px solid var(--panel-border);
  border-radius: 4px;
  font-size: 12px;
}
.chip-att {
  color: var(--orange);
  font-weight: 700;
  font-size: 12px;
}
</style>
