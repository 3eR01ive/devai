<script setup lang="ts">
// Секция ожидания человека (REQ-GATE-01..10) на адресе, который навигация
// уже занимает (#/inbox): все точки, где платформа встала на оператора —
// APPROVAL, WAITING_FOR_INPUT, QA и FAILED (ратифицированное OQ-GATE-2) —
// отдельной нижней группой задачи, остановленные самим оператором
// (PAUSED/BLOCKED), и справочной группой безответных НЕблокирующих
// вопросов (ратифицированное OQ-GATE-1: показаны, платформой на них не
// стоят). Секция наблюдательная: действия (approve, answer, accept/return,
// resume) остаются в CLI — строка несёт точную команду; база read-only
// (OQ-UI-6) закрыта тестом COMMAND_OPERATIONS. Состав и бейдж
// пересчитываются из stored-состояний на каждый load — никакого
// собственного кэша ожидания (REQ-GATE-07).
//
// REQ-GATE-04: клик по строке РАСКРЫВАЕТ предъявление, повторный клик
// сворачивает (детали не развёрнуты по умолчанию). Токены состояний
// сравниваются через общий словарь states.ts, длинные машинные
// тела режутся именованной константой.
import { onMounted, onUnmounted, ref } from "vue";
import { pollMs, setPollMs, POLL_CHOICES, shortId } from "../settings";
import {
  refreshWait,
  setWaitInterval,
  subscribeWait,
  waitError,
  waitSnapshot,
} from "../inbox";
import { TaskStateToken } from "../states";
import WaitTaskHeader from "../components/WaitTaskHeader.vue";
import ConvergenceChart from "../components/ConvergenceChart.vue";
import { stageForRow } from "../convergence_view";

// Сколько символов машинного JSON показывать, когда у quality-отчёта нет
// человеческой части: одна строка-факт, не дамп.
const REPORT_SNIPPET_LEN = 400;

// Снимок секции — общий стор: шапка и вкладка читают
// один и тот же снимок и один интервал опроса (inbox.ts), своего второго
// таймера и второго вызова `inbox_view` здесь нет.
const snap = waitSnapshot;
const error = waitError;
const interval = ref(pollMs());
const expanded = ref<Record<string, boolean>>({});
let unsubscribe: (() => void) | undefined;

function isExpanded(taskId: string): boolean {
  return expanded.value[taskId] === true;
}

function toggle(taskId: string) {
  expanded.value[taskId] = !isExpanded(taskId);
}

function applyInterval(ms: number) {
  setPollMs(ms);
  setWaitInterval(ms);
}

// Срез строки → стадия графика (REQ-CV-13) — правило в convergence_view.ts,
// чтобы его можно было утверждать тестом без сети и без формы гейта.

onMounted(() => {
  unsubscribe = subscribeWait();
});
onUnmounted(() => unsubscribe?.());
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Ожидание оператора</h2>
      <div class="spacer" />
      <label class="muted">опрос каждые
        <select v-model.number="interval" @change="applyInterval(interval)">
          <option v-for="ms in POLL_CHOICES" :key="ms" :value="ms">{{ ms / 1000 }} с</option>
        </select>
      </label>
      <button @click="refreshWait">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>

    <template v-if="snap">
      <div
        v-if="!snap.waiting.length && !snap.human_stopped.length && !snap.notes.length"
        class="empty"
      >
        Платформа нигде не ждёт оператора: ни гейтов, ни припаркованных вопросов, ни заметок
      </div>

      <!-- Гейты: там платформа стоит и ждёт решения. Клик раскрывает
           предъявление, повторный — сворачивает (REQ-GATE-04). -->
      <div v-for="row in snap.waiting" :key="row.task.id" class="panel">
        <div
          class="panel-toggle"
          role="button"
          tabindex="0"
          :aria-expanded="isExpanded(row.task.id)"
          @click="toggle(row.task.id)"
          @keydown.enter.prevent="toggle(row.task.id)"
          @keydown.space.prevent="toggle(row.task.id)"
        >
          <WaitTaskHeader :task="row.task" time-label="ждёт с" />
        </div>
        <div class="body">
          <p style="margin: 0 0 6px">
            От вас ждут: <b>{{ row.demand }}</b>
            <span class="muted mono" style="margin-left: 8px">
              {{ isExpanded(row.task.id) ? "свернуть" : "показать" }}
            </span>
          </p>

          <!-- Мини-график сходимости стадии строки (REQ-CV-12/13): срез
               приходит в фактах снимка; null — только у терминальной
               задачи, у остальных стадия выбирается по состоянию. -->
          <div v-if="row.convergence" class="conv-mini">
            <ConvergenceChart
              :stage="stageForRow(row.convergence, row.task.state).stage"
              :series="stageForRow(row.convergence, row.task.state).series"
              :gap-milli="row.convergence.gap_milli"
              break-on-profile
            />
          </div>

          <template v-if="isExpanded(row.task.id)">
            <!-- APPROVAL: предъявление спеки — абзац и её открытые вопросы
                 раздельными блоками (REQ-GATE-04/05) -->
            <template v-if="row.spec">
              <div v-if="!row.spec.current" class="errorbox" style="margin-bottom: 8px">
                <span class="code">not current</span>
                у задачи нет CURRENT REQUIREMENTS_SPEC; показан последний
                спецификационный артефакт (seq {{ row.spec.artifact.seq }}) — утвердить нельзя
              </div>
              <div class="panel" style="margin: 6px 0">
                <h3>Спека на утверждение · v{{ row.spec.artifact.seq }} ({{ shortId(row.spec.artifact.id) }})</h3>
                <div class="body">
                  <p v-if="row.spec.unavailable_note" class="muted" style="margin: 0 0 6px">
                    абзац: {{ row.spec.unavailable_note }}
                  </p>
                  <p style="margin: 0; white-space: pre-wrap">{{ row.spec.human_paragraph ?? "абзац и бизнес-описание пусты — предъявить нечего" }}</p>
                  <p class="muted" style="margin: 8px 0 0">
                    <template v-if="row.spec.paragraph_source === 'spec'">абзац — human_paragraph спеки</template>
                    <template v-else>абзац подставлен из бизнес-описания задачи (см. метку выше)</template>
                  </p>
                </div>
              </div>
              <div class="panel" style="margin: 6px 0">
                <h3>Открытые вопросы спецификации ({{ row.spec.open_questions.length }}) — вопросы документа, не к вам</h3>
                <div class="body">
                  <p v-if="!row.spec.open_questions.length" class="muted" style="margin: 0">
                    у спеки открытых вопросов нет
                  </p>
                  <ol style="margin: 0; padding-left: 18px">
                    <li v-for="(oq, i) in row.spec.open_questions" :key="i" style="margin-bottom: 6px; white-space: pre-wrap">
                      {{ oq }}
                    </li>
                  </ol>
                  <p class="muted" style="margin: 8px 0 0">
                    Это не platform-вопросы: у них нет id реестра, они не отвечаются
                    <span class="mono">question answer</span> — их разрешение входит в утверждение спеки.
                  </p>
                </div>
              </div>
            </template>
            <p v-else-if="row.task.state === TaskStateToken.Approval" class="muted" style="margin: 6px 0">
              у задачи не опубликовано ни одного спецификационного артефакта — утверждать нечего
            </p>

            <!-- WAITING/гейты: platform-вопросы реестра, блокирующий флаг не теряется -->
            <div v-if="row.open_questions.length" class="panel" style="margin: 6px 0">
              <h3>Вопросы платформы к оператору ({{ row.open_questions.length }})</h3>
              <div class="body">
                <div v-for="q in row.open_questions" :key="q.id" style="margin-bottom: 10px">
                  <div class="rowflex">
                    <span v-if="q.blocking" class="mono" style="color: var(--orange); font-weight: 700">● блокирует</span>
                    <span v-else class="muted mono">○ не блокирует</span>
                    <span class="muted mono">{{ shortId(q.id) }}</span>
                  </div>
                  <p style="margin: 4px 0; white-space: pre-wrap">{{ q.text }}</p>
                  <p v-if="q.context" class="muted" style="margin: 0 0 4px; white-space: pre-wrap">{{ q.context }}</p>
                  <div class="rowflex" v-if="q.options.length">
                    <span v-for="o in q.options" :key="o" class="mono" style="border: 1px solid var(--panel-border); border-radius: 4px; padding: 1px 8px; font-size: 12px">{{ o }}</span>
                  </div>
                </div>
              </div>
            </div>
            <p v-else-if="row.task.state === TaskStateToken.WaitingForInput" class="muted" style="margin: 6px 0">
              Задача в WAITING_FOR_INPUT, но открытых вопросов реестр не показывает —
              это гейт пустого реворка: снять его можно ратификацией (команда ниже).
            </p>

            <!-- QA: отчёт, который просят подписать -->
            <template v-if="row.report">
              <div class="panel" style="margin: 6px 0">
                <h3>Quality-отчёт · seq {{ row.report.seq }} ({{ row.report.contract }})</h3>
                <div class="body">
                  <p v-if="row.report.report_text" style="margin: 0; white-space: pre-wrap">{{ row.report.report_text }}</p>
                  <p v-else class="muted" style="margin: 0">
                    отчёт без человеческой части — машинное тело: <span class="mono">{{ JSON.stringify(row.report.report).slice(0, REPORT_SNIPPET_LEN) }}…</span>
                  </p>
                </div>
              </div>
            </template>
            <p v-else-if="row.report_note" class="muted" style="margin: 6px 0">{{ row.report_note }}</p>
          </template>

          <p class="muted" style="margin: 8px 0 0">Действие: <span class="mono">{{ row.cli_hint }}</span></p>
        </div>
      </div>

      <!-- Остановленные человеком: стоит не платформа, а выбор оператора -->
      <template v-if="snap.human_stopped.length">
        <h3 style="margin: 18px 0 6px" class="muted">Остановленные оператором (платформа на них не стоит)</h3>
        <div v-for="row in snap.human_stopped" :key="row.task.id" class="panel">
          <div class="body">
            <WaitTaskHeader :task="row.task" time-label="с">
              <span class="muted">{{ row.demand }}</span>
              <span class="muted mono">{{ row.cli_hint }}</span>
            </WaitTaskHeader>
            <div v-if="row.convergence" class="conv-mini">
              <ConvergenceChart
                :stage="stageForRow(row.convergence, row.task.state).stage"
                :series="stageForRow(row.convergence, row.task.state).series"
                :gap-milli="row.convergence.gap_milli"
                break-on-profile
              />
            </div>
          </div>
        </div>
      </template>

      <!-- Справочно: безответные неблокирующие заметки (OQ-GATE-1) -->
      <template v-if="snap.notes.length">
        <h3 style="margin: 18px 0 6px" class="muted">
          Справочно: безответные неблокирующие вопросы — заметки, платформа на них не стоит
        </h3>
        <div v-for="n in snap.notes" :key="n.question.id" class="panel">
          <div class="body">
            <div class="rowflex">
              <span class="muted mono">○ не блокирует</span>
              <RouterLink class="title-link" :to="`/task/${n.task.id}`">{{ n.task.display }}</RouterLink>
              · {{ shortId(n.task.id) }}
              <span class="muted mono">{{ shortId(n.question.id) }}</span>
            </div>
            <p style="margin: 4px 0 0; white-space: pre-wrap">{{ n.question.text }}</p>
          </div>
        </div>
      </template>

      <p class="muted">
        Секция наблюдательная: отвечать и утверждать — из CLI
        (<span class="mono">task approve/reject · question answer · task accept/return · task resume · task ratify-empty-rework</span>),
        GUI этой итерации только показывает (OQ-UI-6).
      </p>
    </template>
  </section>
</template>

<style scoped>
h3.muted {
  font-weight: 600;
  letter-spacing: 0.02em;
}
.panel-toggle {
  cursor: pointer;
}
.panel-toggle:hover h3 .title-link,
.panel-toggle:focus-visible h3 .title-link {
  text-decoration: underline;
}
</style>
