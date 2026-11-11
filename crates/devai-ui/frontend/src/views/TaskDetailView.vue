<script setup lang="ts">
// Детали задачи (REQ-UI-05…REQ-UI-08, REQ-UI-10, REQ-UI-11): состояние,
// пайплайн, артефакты, отчёт качества, решения/события, зависимости,
// исполнения и воркспейс — каждое поле из готовой typed-view платформы.
// UI не хранит свою очерёдность стадий, не переприсваивает статусы
// артефактов и не выдумывает строки воркспейсов.
// Дом-гейт (REQ-HOME-04 AC3, ратифицировано OQ-HOME-2/5): детали читаются
// только из АКТИВНОГО дома — ссылка из совмещённого взгляда несёт ?home=.
// Если это не активный дом, экран честно говорит «запись из другого дома»
// и предлагает активацию в один клик; скрытого переключения нет — оператор
// нажимает кнопку. После активации контекст сбрасывается на список
// проектов (без фантомных id между домами).
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, toUiError } from "../api";
import { activateHome } from "../activation";
import { foreignHome as foreignHomeOf } from "../homes-view";
import { fmtTime, pollMs, shortId } from "../settings";
import StateChip from "../components/StateChip.vue";
import SignatureSection from "../components/SignatureSection.vue";
import SpecDiffPanel from "../components/SpecDiffPanel.vue";
import SignatureResultPanel from "../components/SignatureResultPanel.vue";
import { ExecutionStatusToken, LivenessToken, TaskStateToken } from "../states";
import ConvergenceChart from "../components/ConvergenceChart.vue";
import RoleGraphPanel from "../components/RoleGraphPanel.vue";
import QualityGatePanel from "../components/quality/QualityGatePanel.vue";
import type { GateAct } from "../quality_gate_view";
import type {
  ActiveRow,
  Artifact,
  ArtifactContent,
  ArtifactView,
  ConvergenceView,
  Decision,
  DependenciesView,
  ExecutionView,
  ProgressView,
  QualityReportView,
  SignatureView,
  StatusView,
  UiError,
  WorkflowEvent,
  WorkspaceChangesView,
  WorkspaceDiffView,
  WorkspaceFileChange,
} from "../types";

const props = defineProps<{ id: string }>();
const route = useRoute();
const router = useRouter();
const foreignHome = ref<string | null>(null);
const gateError = ref<UiError | null>(null);
const gateBusy = ref(false);

const status = ref<StatusView | null>(null);
const progress = ref<ProgressView | null>(null);
const deps = ref<DependenciesView | null>(null);
const artifacts = ref<ArtifactView[]>([]);
const decisions = ref<Decision[]>([]);
const events = ref<WorkflowEvent[]>([]);
const executions = ref<ExecutionView[]>([]);
// Свежесть снимка живости: он считается текущим только после УСПЕШНОГО
// чтения. Отказ периодического перечитывания не стирает последний факт, но
// помечает его устаревшим — старый «процесс жив» нельзя выдавать за текущий.
const livenessStale = ref(false);
const livenessObservedAt = ref<string | null>(null);
const activeRows = ref<ActiveRow[]>([]);
const report = ref<QualityReportView | null>(null);
const reportError = ref<UiError | null>(null);
// Действия гейта качества (REQ-QG-03 AC4/AC5): отказ — РЯДОМ С КНОПКАМИ,
// а не вверху карточки; обоснование и черновик после отказа сохраняются,
// пока панель не перечитана.
const qualityBusy = ref(false);
const qualityError = ref<UiError | null>(null);
// После УСПЕШНОГО акта черновик/обоснование очищаются (REQ-QG-05 AC3):
// панель пересоздаётся ключом; после отказа ключ не двигается — введённое
// остаётся на месте (REQ-QG-03 AC5).
const qualityEpoch = ref(0);
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
// Секция подписи (REQ-SIGN-01..07): один read `task.signature` на карточку.
const signature = ref<SignatureView | null>(null);
const signatureError = ref<UiError | null>(null);
const signatureBusy = ref(false);
const showDiff = ref(false);
// Сходимость стадий (REQ-CV-12): срез приходит ПОЛЕМ чтения хода задачи —
// отдельной операции нет. UI не считает ни балл, ни режим: рисует готовое.
const convergence = computed<ConvergenceView | null>(() => progress.value?.convergence ?? null);

// Индикатор работы агента (REQ-LV-05): строка карточки отвечает «идёт ли
// работа» фактом, а не догадкой. Слова — платформы (`liveness_line` строки
// `exec.list`; ту же фразу несут task.status и task.progress), окно их не
// перефразирует и вердикт не пересчитывает.
//
// ОДИН снимок факта: строка `exec.list`, перечитываемая таймером. Второй
// снимок того же факта (progress.running_executions из loadAll) с карточки
// убран — два чтения разной свежести давали «исполнение мертво» и «процесс
// жив» на одном экране до ручного «Обновить» (ревью R2, corr/spec-fidelity).
// Статус исполнения сравнивается токеном из словаря states.ts, а не
// литералом: переименование токена в Rust ломает пин-тест, а не фильтр.
const runningRows = computed<ExecutionView[]>(() =>
  executions.value.filter((e) => e.status === ExecutionStatusToken.Running),
);
const runningLines = computed<{ key: string; text: string; dead: boolean }[]>(() =>
  runningRows.value
    .filter((e) => e.liveness_line)
    .map((e) => ({
      key: e.id,
      text: `${e.role_slug}@${e.role_version}: ${e.liveness_line}`,
      // «Мёртво» — только доказанная смерть (REQ-LV-02), и только у СВОЕГО
      // чипа: оформление смерти одной роли не переходит на живую соседнюю.
      dead: e.liveness === LivenessToken.Dead,
    })),
);
const deadRun = computed<boolean>(() => runningLines.value.some((l) => l.dead));
// Явное действие при доказанной смерти (REQ-LV-05 AC3): ТА ЖЕ операция
// платформы, что `devai task run` — она сама снимает доказанно мёртвого
// владельца (reclaim) и запускает по обычным правилам. Окно не собирает эту
// очерёдность и не решает, кому можно запускать.
const restartBusy = ref(false);
const restartLine = ref<string | null>(null);
const restartError = ref<UiError | null>(null);

async function restartDead() {
  if (restartBusy.value) return;
  restartBusy.value = true;
  restartError.value = null;
  restartLine.value = null;
  try {
    // Ответ печатается словами платформы (`line`), не пересказом токена.
    const out = await api.taskRun(props.id, null, false);
    restartLine.value = out.line;
    await loadAll();
  } catch (e) {
    restartError.value = toUiError(e);
  } finally {
    restartBusy.value = false;
  }
}

// Один путь для обоих чтений живости (loadAll и периодическое): успешное
// чтение делает снимок текущим и запоминает его время; отказ оставляет
// прежний снимок и метит его устаревшим.
function observeExecutions(rows: ExecutionView[]) {
  executions.value = rows;
  livenessStale.value = false;
  livenessObservedAt.value = new Date().toISOString();
}

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
    observeExecutions(execs);
    activeRows.value = snap.rows.filter((r) => r.task_id === props.id || r.task_id.startsWith(props.id));
    await loadEvents();
    await loadReport();
    await loadWorkspace();
    await loadSignature();
  } catch (e) {
    error.value = toUiError(e);
  }
}

// Один read на всю секцию подписи (REQ-SIGN-07): права, спека, ревьювер,
// дифф и самый новый факт подписи. UI ничего не собирает сам.
async function loadSignature() {
  signatureError.value = null;
  try {
    signature.value = await api.taskSignature(props.id);
  } catch (e) {
    signature.value = null;
    signatureError.value = toUiError(e);
  }
}

// Подпись/отклонение — те же акты платформы, что `devai task approve/reject`
// (REQ-SIGN-05): окно только зовёт мутацию и перечитывает карточку.
async function applySignature(kind: "approve" | "reject", rationale: string) {
  if (signatureBusy.value) return;
  signatureBusy.value = true;
  signatureError.value = null;
  try {
    if (kind === "approve") {
      await api.taskApprove(props.id, rationale);
    } else {
      await api.taskReject(props.id, rationale);
    }
    await loadAll();
  } catch (e) {
    signatureError.value = toUiError(e);
  } finally {
    signatureBusy.value = false;
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
// (REQ-QH-01): представление и действия живут в `QualityGatePanel`; здесь
// только загрузка и один путь мутации.

// Гейт качества: те же два акта, что `devai task accept|return` (REQ-QG-01).
// Доступность приходит полем `status.quality_gate` — окно её не вычисляет.
async function applyQuality(kind: GateAct, rationale: string) {
  if (qualityBusy.value) return;
  qualityBusy.value = true;
  qualityError.value = null;
  try {
    const next =
      kind === "accept"
        ? await api.taskAccept(props.id, rationale)
        : await api.taskReturn(props.id, rationale);
    status.value = next;
    qualityEpoch.value += 1;
    await loadAll();
  } catch (e) {
    qualityError.value = toUiError(e);
  } finally {
    qualityBusy.value = false;
  }
}

// Обоснование решения читается РЯДОМ С СОБЫТИЕМ в истории (REQ-QG-07 AC2),
// а не только в раскрытом payload: accept/return кладут rationale в событие,
// телефонный актор уже назван устройством.
function eventRationale(ev: WorkflowEvent): string | null {
  const p = ev.payload as Record<string, unknown> | null | undefined;
  const value = p?.rationale ?? p?.qa_return_rationale;
  return typeof value === "string" && value.trim() ? value : null;
}

async function openContent(a: Artifact) {
  contentFor.value = a.id;
  content.value = null;
  try {
    content.value = await api.artifactContent(a.id);
  } catch (e) {
    error.value = toUiError(e);
  }
}

async function gateAndLoad() {
  // Источником дома для записи является блок совмещённого взгляда (?home
  // из tasks_all). Чужой дом НЕ читаем этим api-хендлом: фантомных id
  // между домами не допускаем (ратифицировано OQ-HOME-5).
  const home = (route.query.home as string) ?? null;
  if (home) {
    try {
      const s = await api.sessionInfo();
      foreignHome.value = foreignHomeOf(home, s.home);
      if (foreignHome.value) return;
    } catch (e) {
      error.value = toUiError(e);
      return;
    }
  }
  await loadAll();
}
// Живость перечитывается ПОСТОЯННО — из нашего же окна, без системных
// шедулеров и сторожевых скриптов (ратифицированный ответ на вопрос цикла).
// Дешёвое чтение `exec.list` идёт на КАЖДОМ тике для любой незавершённой
// задачи — в том числе когда RUNNING-строк сейчас нет (строка может
// появиться). Терминальная задача без RUNNING-строк не опрашивается: новых
// строк она не родит.
let livenessTimer: number | undefined;

async function refreshLiveness() {
  if (foreignHome.value || !status.value) return;
  // Терминальная задача МОЖЕТ держать RUNNING-строку — платформа
  // классифицирует этот класс отдельно (stale = terminal || dead,
  // devai-api/src/lib.rs:674-675). Пока такая строка в снимке есть, опрос
  // не прекращается: иначе смерть владельца навсегда осталась бы «процесс
  // жив» — старый факт выдавался бы за текущий. Нет RUNNING-строк —
  // опрашивать нечего.
  if (progress.value?.pipeline.terminal && runningRows.value.length === 0) return;
  try {
    observeExecutions(await api.executionsList(props.id));
  } catch {
    // Карточку отказ отдельного опроса не рушит, но и НЕ прячется: последний
    // снимок остаётся на экране помеченным как устаревший (время успешного
    // чтения рядом), иначе старый «процесс жив» читался бы как текущий факт.
    livenessStale.value = true;
  }
}

onMounted(async () => {
  await gateAndLoad();
  // Таймер заводится при монтировании и снимается onUnmounted: компонентные
  // тесты идут в jsdom, где `window` есть, поэтому интервал там действительно
  // ставится — его снимает размонтирование, а не окружение.
  if (typeof window !== "undefined") {
    livenessTimer = window.setInterval(refreshLiveness, pollMs());
  }
});
onUnmounted(() => {
  if (livenessTimer && typeof window !== "undefined") window.clearInterval(livenessTimer);
});

async function activateForeignHome() {
  if (!foreignHome.value) return;
  gateBusy.value = true;
  gateError.value = null;
  try {
    // R1 quality return: тот же единый путь, что и в реестре домов, —
    // активация публикует свежий SessionInfo в стор окна, иначе
    // статус-бар/деградированный баннер держали бы старый дом.
    await activateHome(foreignHome.value);
    // OQ-HOME-5: после переключения — сброс контекста на список проектов.
    await router.push("/projects");
  } catch (e) {
    gateError.value = toUiError(e);
  } finally {
    gateBusy.value = false;
  }
}
</script>

<template>
  <section v-if="foreignHome" class="panel">
    <h3>Запись из другого дома</h3>
    <div class="body">
      <p class="muted" style="margin: 0 0 8px">
        Дом <span class="mono">{{ foreignHome }}</span> не является активным.
        Детали задачи читаются только из активного дома — скрытого
        переключения ради одного просмотра нет (ратифицировано OQ-HOME-2).
      </p>
      <div v-if="gateError" class="errorbox">
        <span class="code">{{ gateError.code }}</span>{{ gateError.message }}
      </div>
      <div class="rowflex">
        <button :disabled="gateBusy" @click="activateForeignHome">
          {{ gateBusy ? "Активация…" : "Активировать этот дом" }}
        </button>
        <RouterLink to="/tasks" style="margin-left: 10px">Назад к задачам</RouterLink>
      </div>
    </div>
  </section>

  <section v-if="error" class="errorbox">
    <span class="code">{{ error.code }}</span>{{ error.message }}
  </section>

  <section v-if="status">
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">{{ status.task.display }}</h2>
      <StateChip :token="status.task.state" />
      <StateChip v-if="status.task.return_state" :token="status.task.return_state" />
      <!-- Индикатор работы (REQ-LV-05): одна строка, без раскрытия деталей. -->
      <span
        v-for="line in runningLines"
        :key="line.key"
        class="chip-liveness"
        :class="{ dead: line.dead, stale: livenessStale }"
      >{{ line.text }}</span>
      <!-- Перечитать живость не удалось: снимок остаётся, но говорится прямо,
           что он устарел и когда читался последний раз успешно. -->
      <span v-if="livenessStale" class="chip-liveness stale">
        данные устарели · последнее успешное чтение {{ fmtTime(livenessObservedAt) }}
      </span>
      <div class="spacer" />
      <span class="mono muted">{{ status.task.id }}</span>
      <button @click="loadAll">Обновить</button>
    </div>
    <p
      v-if="status.task.display !== status.task.title"
      class="muted mono"
      style="margin: 0 0 10px"
    >заголовок создавшего: {{ status.task.title }}</p>
    <!-- Доказанная смерть: явное действие В ОКНЕ, а не подсказка с CLI
         (REQ-LV-05 AC3). Ответ платформы — её собственные слова. -->
    <div v-if="deadRun" class="rowflex" style="margin: 0 0 10px">
      <button :disabled="restartBusy" @click="restartDead">
        {{ restartBusy ? "Перезапуск…" : "перезапустить" }}
      </button>
      <span v-if="restartLine" class="muted">{{ restartLine }}</span>
    </div>
    <div v-if="restartError" class="errorbox" style="margin-bottom: 10px">
      <span class="code">{{ restartError.code }}</span>{{ restartError.message }}
    </div>
    <p class="muted" style="white-space: pre-wrap">{{ status.task.business_description }}</p>

    <!-- ЭКРАН ПОДПИСИ (REQ-SIGN-01): первым содержательным блоком на
         APPROVAL — до формальной структуры панелей. Вне APPROVAL секции и
         её пустой рамки нет. Данные — одним task.signature. -->
    <div v-if="signatureError" class="errorbox">
      <span class="code">{{ signatureError.code }}</span>{{ signatureError.message }}
    </div>
    <SignatureSection
      v-if="status.task.state === TaskStateToken.Approval && signature"
      :signature="signature"
      :busy="signatureBusy"
      :error="signatureError"
      @apply="applySignature"
      @open-diff="showDiff = true"
    />
    <SignatureResultPanel
      v-if="signature && signature.signature"
      :fact="signature.signature"
    />
    <SpecDiffPanel
      v-if="showDiff && signature && signature.previous"
      :previous="signature.previous"
    />

    <!-- ПУТЬ ЗАДАЧИ (REQ-RG-06): граф ролей и этапов заменяет прежнюю
         линейку стадий. Данные — `progress.graph` одним чтением; окно не
         хранит порядка этапов, таблицы «состояние → узел» и подписей. -->
    <RoleGraphPanel
      v-if="progress?.graph"
      :graph="progress.graph"
      :snapshot-at="progress.snapshot_at"
    />
    <!-- Счётчик попыток роли без прогресса (REQ-AL-07): виден и НИЖЕ
         предела, числа — одной строкой платформы (не собираются здесь). -->
    <div v-if="progress?.attempt_limit" class="panel">
      <div class="body">
        <p v-if="progress.attempt_limit" class="muted mono" style="margin: 0">
          роль {{ progress.attempt_limit.role }} · {{ progress.attempt_limit.line }}
        </p>
      </div>
    </div>

    <!-- Сходимость стадий (REQ-CV-12/13): по графику на каждую стадию с
         кругами, порядок и значения — от платформы (spec_review → quality).
         Срез пришёл полем progress; без кругов — честная строка без осей. -->
    <div class="panel">
      <h3>Сходимость стадий</h3>
      <div class="body">
        <template v-if="convergence">
          <p v-if="!convergence.stages.length" class="muted" style="margin: 0">
            кругов пока нет — у задачи не опубликовано ни отчёта ревью, ни QA-отчёта
          </p>
          <template v-else>
            <p class="muted" style="margin: 0 0 8px">
              шаг прироста: <span class="mono">{{ convergence.gap_milli }}</span> тысячных
            </p>
            <div v-for="s in convergence.stages" :key="s.stage" class="conv-stage">
              <ConvergenceChart
                :stage="s.stage"
                :series="s.series"
                :gap-milli="convergence.gap_milli"
                break-on-profile
              />
              <p class="muted mono stage-caption">
                время стадии
                {{ s.stage_total_seconds === null ? "неизвестно" : `${s.stage_total_seconds} с` }} ·
                кругов {{ s.counters.rounds_total }} · не измерено
                {{ s.counters.rounds_unmeasured }} · без времени
                {{ s.counters.rounds_without_time }}<template v-if="s.stage === 'spec_review'">
                  · версий спеки без ревью {{ s.counters.spec_versions_without_review }}</template>
              </p>
            </div>
          </template>
        </template>
        <p v-else class="muted" style="margin: 0">
          у завершённой задачи ряд не отдаётся
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
            <span v-if="r.note" class="chip-att">{{ r.note }}</span>
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
                  <td class="mono" data-label="Что" data-mobile="head">{{ f.status }}</td>
                  <td class="mono" data-label="Путь" data-mobile="title">{{ f.path }}</td>
                  <td data-label="Действия"><button @click="openWorkspaceDiff(f)">дифф</button></td>
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
          <RouterLink class="title-link" :to="`/task/${d.depends_on_id}`">{{ d.depends_on_display }}</RouterLink>
          <StateChip :token="d.depends_on_state" />
        </div>
        <div v-for="d in deps.incoming" :key="d.id" class="rowflex" style="margin-bottom: 4px">
          <span class="mono muted">← {{ d.kind }}</span>
          <RouterLink class="title-link" :to="`/task/${d.task_id}`">{{ d.task_display }}</RouterLink>
          <StateChip :token="d.task_state" />
        </div>
      </div>
    </div>

    <div class="panel">
      <h3>Артефакты (название · версия · статус — статусы хранимые)</h3>
      <div class="body">
        <table class="data">
          <thead>
            <tr><th>Артефакт</th><th>Хранимое имя</th><th>Статус</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="a in artifacts" :key="a.id">
              <!-- REQ-NM-01: первой — готовая человеческая подпись; ключ вида
                   и id уходят в приглушённую вторую позицию. -->
              <td data-label="Артефакт" data-mobile="title">
                {{ a.display }}
                <span class="muted mono">{{ a.kind }} · {{ shortId(a.id) }}</span>
              </td>
              <td data-label="Хранимое имя">{{ a.name }}</td>
              <td data-label="Статус" data-mobile="head"><StateChip :token="a.status" /></td>
              <td data-label="Действия"><button @click="openContent(a)">содержимое</button></td>
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
        <!-- Отчёт + интерактивность + действия гейта (REQ-QG-01..09):
             сохранённые группы/счётчики, раскрытие пунктов, фильтр,
             черновик возврата и удержание кнопок. Ветки представления и
             правила черновика — отдельными чистыми модулями. -->
        <QualityGatePanel
          :key="qualityEpoch"
          :report="report"
          :report-error="reportError"
          :gate="status.quality_gate"
          :busy="qualityBusy"
          :error="qualityError"
          @apply="applyQuality"
        />
      </div>
    </div>

    <div class="panel">
      <h3>Исполнения ролей (exec.list; живость — та же проверка, что на карточке)</h3>
      <div class="body">
        <table class="data">
          <thead>
            <tr><th>Роль@вер</th><th>Агент</th><th>Статус</th><th>Живость</th><th>Старт / финиш</th><th>Класс сбоя</th></tr>
          </thead>
          <tbody>
            <tr v-for="e in executions" :key="e.id">
              <td class="mono" data-label="Роль@вер" data-mobile="title">
                {{ e.role_display }}
                <span class="muted">{{ e.role_slug }}@{{ e.role_version }} · попытка {{ e.attempt ?? 1 }}</span>
              </td>
              <td data-label="Агент">{{ e.agent_name ?? "—" }}</td>
              <td data-label="Статус" data-mobile="head"><StateChip :token="e.status" /></td>
              <td data-label="Живость" data-mobile="head">
                <template v-if="e.liveness">
                  <StateChip :token="e.liveness" />
                  <span class="muted"> {{ e.liveness_line }}</span>
                </template>
                <span v-else class="muted">—</span>
              </td>
              <td class="mono muted" data-label="Старт / финиш">{{ fmtTime(e.started_at) }} → {{ fmtTime(e.finished_at) }}</td>
              <td class="mono muted" data-label="Класс сбоя">{{ e.failure_class ?? "—" }}</td>
            </tr>
          </tbody>
        </table>
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
          <!-- Строка подписи без rationale — валидный факт «без обоснования»
               (REQ-SIGN-06 AC6): видимый маркер вместо спрятанного <details>.
               Остальные виды решений рендерятся как раньше. -->
          <p
            v-else-if="d.kind === 'requirements_approval' || d.kind === 'requirements_rejection'"
            class="muted"
            style="margin: 2px 0 0"
          >без обоснования</p>
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
          <thead>
            <tr><th>Событие</th><th>Актор</th><th>Время</th><th>Обоснование</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="ev in events" :key="ev.id">
              <td class="mono" data-label="Событие" data-mobile="title">{{ ev.kind }}</td>
              <td class="muted" data-label="Актор">{{ ev.actor }}</td>
              <td class="mono muted" data-label="Время" data-mobile="head">{{ fmtTime(ev.created_at) }}</td>
              <!-- REQ-QG-07 AC2: обоснование решения видно РЯДОМ С событием,
                   не только в раскрытом payload. -->
              <td class="muted event-rationale" data-label="Обоснование">{{ eventRationale(ev) ?? "—" }}</td>
              <td data-label="Данные">
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
  <section v-else-if="!error && !foreignHome" class="empty">Загрузка…</section>
</template>

<style scoped>
/* Подпись «время стадии…» — служебная: на десктопе 11px, на телефоне и
   планшете не мельче 12px (REQ-RESP-08 AC3). */
.stage-caption {
  margin: 2px 0 0;
  font-size: 11px;
}
@media (max-width: 1199px) {
  .stage-caption {
    font-size: 12px;
  }
}
/* Индикатор работы агента (REQ-LV-05): живёт в строке заголовка, перенос
   запрещён — «одна строка карточки», без раскрытия и без скролла. */
.chip-liveness {
  padding: 2px 8px;
  border: 1px solid var(--slate);
  border-radius: 4px;
  font-size: 12px;
  color: var(--cream);
  white-space: nowrap;
}
.chip-liveness.dead {
  border-color: #b08a4f;
  color: #e8caa0;
  font-weight: 700;
}
/* Устаревший снимок: пунктир вместо сплошной рамки — факт на экране есть, но
   он не текущий, и окно об этом говорит словом, а не цветом. */
.chip-liveness.stale {
  border-style: dashed;
  font-style: italic;
  opacity: 0.8;
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
.event-rationale {
  white-space: pre-wrap;
  max-width: 420px;
}
</style>
