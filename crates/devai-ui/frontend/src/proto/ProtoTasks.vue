<script setup lang="ts">
// Новый список задач (по макету): путь ролей в строке, группы по смыслу,
// состояние раннера и расход токенов. Старый список остаётся рядом.
import { computed, nextTick, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { api, toUiError } from "../api";
import type { ProgressView, Project, Task, TaskUsageView, UiError } from "../types";
import "./proto.css";

const router = useRouter();
const tasks = ref<Task[]>([]);

// ── постановка новой задачи прямо из списка ──────────────────────────────
// Форма живёт в самой вкладке, без модальных окон: название, постановка и
// проект (если проектов несколько). Созданная задача открывается карточкой —
// человек сразу видит, что поставил.
const creating = ref(false);
const newTitle = ref("");
const newBody = ref("");
const newProject = ref<string>("");
const createBusy = ref(false);
const projects = ref<Project[]>([]);
const canCreate = computed(() => newTitle.value.trim().length > 0 && !createBusy.value);

function openCreate() {
  creating.value = true;
  newTitle.value = "";
  newBody.value = "";
  newProject.value = projects.value.length === 1 ? projects.value[0].id : "";
  if (!projects.value.length) {
    void api.projectsList().then((p) => {
      projects.value = p;
      if (p.length === 1) newProject.value = p[0].id;
    }).catch(() => undefined);
  }
}

async function createTask() {
  if (!canCreate.value) return;
  createBusy.value = true;
  error.value = null;
  try {
    const t = await api.taskNew(newTitle.value.trim(), newBody.value, newProject.value || null);
    creating.value = false;
    await router.push(`/proto/task/${t.id}`);
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    createBusy.value = false;
  }
}
const progress = ref<Record<string, ProgressView>>({});
const usage = ref<Record<string, TaskUsageView>>({});
const error = ref<UiError | null>(null);
const loading = ref(true);
// Закрытые показываются по требованию: по умолчанию свернуты, не выброшены.
const showClosed = ref(false);

const TERMINAL = new Set(["ACCEPTED", "CANCELLED"]);
const GROUPS: [string, string, string[]][] = [
  ["you", "Ждут тебя", ["WAITING_FOR_INPUT", "APPROVAL"]],
  ["work", "Идёт работа", ["STATEMENT", "ANALYSIS", "REVIEW", "IMPLEMENTATION", "QA", "REWORK"]],
  ["stop", "Стоят", ["BLOCKED", "PAUSED", "FAILED"]],
  ["draft", "Черновики", ["DRAFT"]],
  // Закрытые дела никуда не деваются: они есть в списке, но по умолчанию
  // свернуты одной группой — не мешают, пока не нужны.
  ["done", "Закрытые", ["ACCEPTED", "CANCELLED", "REJECTED"]],
];
const CLOSED_GROUP = "done";
const SHORT: Record<string, string> = {
  STATEMENT: "постановка", ANALYSIS: "анализ", REVIEW: "ревью",
  APPROVAL: "подпись", IMPLEMENTATION: "код", QA: "приёмка",
};
const STATE_WORD: Record<string, string> = {
  DRAFT: "черновик", STATEMENT: "у постановщика", ANALYSIS: "анализ", REVIEW: "ревью",
  APPROVAL: "ждёт подписи", IMPLEMENTATION: "код", QA: "приёмка", REWORK: "доработка",
  WAITING_FOR_INPUT: "ждёт человека", PAUSED: "на паузе", BLOCKED: "остановлена",
  FAILED: "упала", ACCEPTED: "принята", CANCELLED: "отменена",
};
const TONE: Record<string, string> = {
  WAITING_FOR_INPUT: "b-human", APPROVAL: "b-human", BLOCKED: "b-stop", FAILED: "b-stop",
  PAUSED: "b-stop", REWORK: "b-now", QA: "b-pass", DRAFT: "b-mute",
};

function age(iso: string): string {
  const s = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 1000));
  if (s < 60) return `${s} с`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} мин`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h} ч ${m % 60} мин` : `${Math.floor(h / 24)} д ${h % 24} ч`;
}

function tok(n: number): string {
  if (!n) return "0";
  if (n >= 1e9) return (n / 1e9).toFixed(2) + " млрд";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " млн";
  if (n >= 1e3) return Math.round(n / 1e3) + " тыс";
  return String(n);
}

/**
 * Расход строки списка: сумма известных прогонов, «—» когда известных нет, а
 * НЕИЗВЕСТНЫЕ есть (их число стоит рядом) — неизвестный расход не выдаётся за
 * ноль (REQ-USG-02 AC1).
 */
function spentText(spent: number, unknown: number): string {
  if (spent === 0 && unknown > 0) return "—";
  return tok(spent);
}

const rowOf = (t: Task) => {
  const p = progress.value[t.id];
  const stages = (p?.graph?.stages ?? []).map((s) => ({
    short: SHORT[s.state] ?? s.label, status: s.status, gate: s.gate,
  }));
  const running = p?.running_executions ?? [];
  let kind = "idle";
  let word = "прогона нет";
  if (running.length) {
    kind = "run";
    // Платформа уже посчитала прошедшее время — окно его не пересчитывает.
    const sec = running[0].elapsed_seconds ?? 0;
    const m = Math.max(0, Math.round(sec / 60));
    word = `идёт · ${m < 60 ? m + " мин" : Math.floor(m / 60) + " ч " + (m % 60) + " мин"}`;
  } else if (t.state === "APPROVAL") { kind = "human"; word = "ждёт подписи · " + age(t.updated_at); }
  else if (t.state === "WAITING_FOR_INPUT") { kind = "human"; word = "ждёт ответа · " + age(t.updated_at); }
  else if (["BLOCKED", "PAUSED", "FAILED"].includes(t.state)) { kind = "stop"; word = STATE_WORD[t.state] + " · " + age(t.updated_at); }
  else if (["STATEMENT", "ANALYSIS", "REVIEW", "IMPLEMENTATION", "QA", "REWORK"].includes(t.state)) {
    kind = "warn"; word = "прогона нет · " + age(t.updated_at);
  } else if (t.state === "DRAFT") { kind = "idle"; word = "не запускалась"; }
  else if (t.state === "ACCEPTED") { kind = "done"; word = "принята · " + age(t.updated_at); }
  else if (t.state === "REJECTED") { kind = "done"; word = "отклонена · " + age(t.updated_at); }
  else if (t.state === "CANCELLED") { kind = "done"; word = "отменена · " + age(t.updated_at); }
  else { word = "прогона нет · " + age(t.updated_at); }
  return {
    t, stages, kind, word,
    state: STATE_WORD[t.state] ?? t.state.toLowerCase(),
    tone: TONE[t.state] ?? "b-mute",
    u: usage.value[t.id],
    id: t.id.slice(0, 8),
    link: `/proto/task/${t.id}`,
  };
};

// Кто идёт ПРЯМО СЕЙЧАС — отдельной группой сверху: это вопрос «что работает»,
// а не «в какой стадии». Из стадийных групп такие строки убраны, иначе одно
// дело стояло бы дважды.
const runningIds = computed(
  () =>
    new Set(
      Object.entries(progress.value)
        .filter(([, p]) => (p.running_executions ?? []).length > 0)
        .map(([id]) => id),
    ),
);

const groups = computed(() => {
  const run = {
    key: "run",
    title: "Идёт сейчас",
    items: tasks.value.filter((t) => runningIds.value.has(t.id)).map(rowOf),
  };
  const rest = GROUPS.map(([key, title, states]) => ({
    key, title,
    items: tasks.value.filter((t) => states.includes(t.state)).map(rowOf),
  }))
    .filter((g) => g.items.length)
    .filter((g) => showClosed.value || g.key !== CLOSED_GROUP)
    .map((g) => ({ ...g, items: g.items.filter((r) => !runningIds.value.has(r.t.id)) }))
    .filter((g) => g.items.length);
  return run.items.length ? [run, ...rest] : rest;
});

const closedCount = computed(
  () => tasks.value.filter((t) => GROUPS.find(([k]) => k === CLOSED_GROUP)![2].includes(t.state)).length,
);

async function toggleClosed() {
  showClosed.value = !showClosed.value;
  // Провал закрытых дел читается только когда их открыли: список не платит
  // за десятки закрытых задач на каждом входе.
  if (showClosed.value) await loadDetail(tasks.value.filter((t) => TERMINAL.has(t.state)));
  if (showClosed.value) await nextTick();
}

const counts = computed(() => ({
  all: tasks.value.length,
  open: tasks.value.filter((t) => !TERMINAL.has(t.state)).length,
}));

/// Ход и расход пачкой, пятью параллельными потоками: строка без пути честнее
/// выдуманного, поэтому отказ отдельной задачи не валит весь список.
async function loadDetail(list: Task[]) {
  const queue = list.filter((t) => !progress.value[t.id]).slice();
  const workers = Array.from({ length: 5 }, async () => {
    while (queue.length) {
      const t = queue.shift()!;
      try {
        const [p, u] = await Promise.all([api.taskProgress(t.id), api.taskUsage(t.id)]);
        progress.value = { ...progress.value, [t.id]: p };
        usage.value = { ...usage.value, [t.id]: u };
      } catch {
        /* строка останется без пути */
      }
    }
  });
  await Promise.all(workers);
}

async function load() {
  loading.value = true;
  error.value = null;
  try {
    tasks.value = await api.tasksList();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    loading.value = false;
  }
  // Закрытые читаются только когда их открыли.
  const open = tasks.value.filter((t) => !TERMINAL.has(t.state));
  await loadDetail(showClosed.value ? tasks.value : open);
}

onMounted(load);
</script>

<template>
  <section class="p-root">
    <header class="p-head">
      <h1>
        Задачи <span>· открытых {{ counts.open }} из {{ counts.all }} · живой дом</span>
      </h1>
      <div class="p-groups">
        <span v-for="g in groups" :key="g.key" class="p-runchip" :class="g.key === 'you' ? 'human' : g.key === 'stop' ? 'stop' : g.key === 'work' ? 'warn' : ''">
          {{ g.title }} · {{ g.items.length }}
        </span>
        <!-- закрытые дела не выброшены: они здесь, одной кнопкой -->
        <button class="p-runchip p-toggle" :class="{ on: showClosed }" @click="toggleClosed">
          {{ showClosed ? 'скрыть закрытые' : 'закрытые' }} · {{ closedCount }}
        </button>
        <button class="p-new" :class="{ on: creating }" @click="creating ? (creating = false) : openCreate()">
          {{ creating ? 'отменить' : '+ задача' }}
        </button>
      </div>
    </header>

    <!-- постановка: название, сама постановка, проект — без модальных окон -->
    <div v-if="creating" class="p-newform">
      <div class="p-line">
        <span class="lab">название</span>
        <input v-model="newTitle" class="t" placeholder="что нужно сделать" spellcheck="false"
               @keydown.enter="createTask" />
      </div>
      <div class="p-line top">
        <span class="lab">постановка</span>
        <textarea v-model="newBody" rows="5" spellcheck="false"
                  placeholder="постановка: потребность, что считается сделанным, границы" />
      </div>
      <div v-if="projects.length > 1" class="p-line">
        <span class="lab">проект</span>
        <select v-model="newProject" class="t">
          <option value="">без проекта</option>
          <option v-for="p in projects" :key="p.id" :value="p.id">{{ p.name }}</option>
        </select>
      </div>
      <div class="p-formacts">
        <button class="p-create" :disabled="!canCreate" @click="createTask">
          {{ createBusy ? 'создаю…' : 'Поставить задачу' }}
        </button>
        <span class="hint">после создания откроется карточка; путь начнётся с постановщика</span>
      </div>
    </div>

    <div v-if="error" class="p-err">{{ error.code }} — {{ error.message }}</div>
    <div v-else-if="loading" class="p-empty">читаю задачи…</div>

    <template v-for="g in groups" :key="g.key">
      <div class="p-grp" :class="g.key">
        <span class="t">{{ g.title }}</span><span class="n">{{ g.items.length }}</span><span class="bar" />
      </div>
      <RouterLink
        v-for="r in g.items"
        :key="r.t.id"
        :to="r.link"
        class="p-row"
        :class="'s-' + r.t.state"
        style="text-decoration: none"
      >
        <div>
          <div class="p-name">{{ r.t.display }}</div>
          <div class="p-ident">
            <span>{{ r.id }}</span>
            <span v-if="r.u" :title="r.u.line">
              {{ spentText(r.u.spent, r.u.unknown) }}<template v-if="r.u.limit != null"> из {{ tok(r.u.limit) }}</template>
              токенов<template v-if="r.u.unknown > 0"> · неизвестно у {{ r.u.unknown }}</template>
            </span>
          </div>
        </div>
        <div class="p-tl">
          <div
            v-for="(s, i) in r.stages"
            :key="i"
            class="p-step"
            :class="[s.status, { gate: s.gate }]"
          >
            <div v-if="i" class="rail" />
            <div class="p-node" />
            <div class="cap">{{ s.short }}</div>
          </div>
          <div v-if="!r.stages.length" class="p-empty" style="padding: 0">путь недоступен</div>
        </div>
        <div class="p-side">
          <span class="p-state" :class="r.tone">{{ r.state }}</span>
          <span class="p-runchip" :class="r.kind"><i />{{ r.word }}</span>
        </div>
      </RouterLink>
    </template>
    <div v-if="!loading && !tasks.length && !error" class="p-empty">задач нет</div>
  </section>
</template>
