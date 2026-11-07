<script setup lang="ts">
// Реестр домов (REQ-HOME-01/03/06/07/09): список известных домов как
// данные платформы, добавление/включение без рестарта процесса и сделка
// активного дома одним кликом (орган записи утверждённой формы OQ-HOME-6).
// Удаление из реестра = выключение (ратифицировано OQ-HOME-3): запись
// остаётся с атрибутами. Выбор папки — нативный диалог (plugin-dialog,
// R1 оператора); текстовое поле пути остаётся ручным fallback, список
// недавно добавленных — как был. Диалог — не дверь в данные: он возвращает
// строку пути, а регистрирует её по-прежнему операция home.add.
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { api, toUiError } from "../api";
import { activateHome } from "../activation";
import { sessionStore } from "../session";
import {
  RECENT_HEADING,
  canActivate,
  homeIssue,
  homeIssueText,
  homeState,
  noActiveHomeNote,
  projectLabelNote,
  recentHomeLabel,
  recentHomes,
  schemaLabel,
} from "../homes-view";
import { fmtTime } from "../settings";
import StateChip from "../components/StateChip.vue";
import type { HomeRecord, UiError } from "../types";

const router = useRouter();
const homes = ref<HomeRecord[]>([]);
const error = ref<UiError | null>(null);
const notice = ref<string | null>(null);
const loading = ref(true);
const addPath = ref("");

async function load() {
  loading.value = true;
  error.value = null;
  try {
    homes.value = await api.homesList();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

async function add(path: string) {
  error.value = null;
  notice.value = null;
  try {
    await api.homesAdd(path.trim());
    addPath.value = "";
    await load();
  } catch (e) {
    error.value = toUiError(e);
  }
}

// Нативный диалог выбора папки дома. Отмена — штатный null, без ошибки.
// Вне Tauri (чистый браузер через vite) вызов бросается — честно говорим
// это и оставляем ручной ввод: поле пути и recent-homes не регрессируют.
async function pick() {
  error.value = null;
  notice.value = null;
  let picked: string | null;
  try {
    picked = await open({
      directory: true,
      multiple: false,
      title: "Каталог дома проекта (.devai)",
    });
  } catch (e) {
    notice.value = `Нативный диалог недоступен в этой сессии (${String(e)}) — введите путь вручную.`;
    return;
  }
  if (typeof picked === "string" && picked.trim()) {
    await add(picked);
  }
}

async function toggle(h: HomeRecord) {
  error.value = null;
  try {
    homes.value = await api.homesSetEnabled(h.path, !h.enabled);
    // Ratified OQ-HOME-8: switching the active home off moves the stored
    // marker (or leaves no active home at all), so the window must re-read
    // the session — the status bar and the no-active banner branch on it.
    await sessionStore.refresh();
  } catch (e) {
    error.value = toUiError(e);
  }
}

async function activate(h: HomeRecord) {
  error.value = null;
  notice.value = null;
  try {
    // R1 quality return: активация идёт единым путём activateHome, который
    // публикует свежий SessionInfo в стор окна — статус-бар и все
    // потребители сессии видят новый активный дом без перезагрузки.
    await activateHome(h.path);
    // Ратифицировано OQ-HOME-5: после переключения контекст вью
    // сбрасывается на список проектов; фантомных id между домами нет.
    await load();
    router.push("/projects");
  } catch (e) {
    error.value = toUiError(e);
  }
}
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Дома проектов</h2>
      <div class="spacer" />
      <button @click="load">Обновить</button>
    </div>
    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>
    <div v-if="notice" class="muted" style="margin-bottom: 8px">{{ notice }}</div>
    <!-- Ratified OQ-HOME-8: the "no active home" state is said plainly and
         both ways out (enable / add) are on this very screen. -->
    <div v-if="noActiveHomeNote(homes)" class="panel" style="margin-bottom: 12px">
      <div class="body">{{ noActiveHomeNote(homes) }}</div>
    </div>

    <div class="panel">
      <div class="body">
        <div class="rowflex">
          <input
            v-model="addPath"
            class="mono"
            placeholder="/path/to/project/.devai"
            style="flex: 1"
            @keyup.enter="add(addPath)"
          />
          <button @click="pick">Выбрать папку…</button>
          <button :disabled="!addPath.trim()" @click="add(addPath)">Добавить дом</button>
        </div>
        <p class="muted" style="margin: 8px 0 0">
          Регистрируется путь; данные дома не копируются и не мигрируются.
          Путь без devai.db отклоняется разбором платформы.
        </p>
        <div v-if="recentHomes(homes).length" style="margin-top: 8px">
          <span class="muted">{{ RECENT_HEADING }}</span>
          <button
            v-for="h in recentHomes(homes)"
            :key="'recent-' + h.path"
            style="margin-left: 6px"
            :title="h.enabled ? 'Дом уже в реестре — повторное добавление сохранит дату' : 'Включить выключенный дом'"
            @click="add(h.path)"
          >
            {{ recentHomeLabel(h) }}
          </button>
        </div>
      </div>
    </div>

    <div v-if="!loading && homes.length === 0" class="empty">
      Реестр пуст — это валидное состояние (однодомный режим). Первый старт
      платформы регистрирует дом запуска сам.
    </div>
    <table v-if="homes.length" class="data">
      <thead>
        <tr>
          <th>Проект</th>
          <th>Путь</th>
          <th>Добавлен</th>
          <th>Состояние</th>
          <th>Схема</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="h in homes" :key="h.path">
          <td>
            <b>{{ h.project }}</b>
            <div
              v-if="projectLabelNote(h)"
              class="muted"
              style="font-size: 12px"
            >
              {{ projectLabelNote(h) }}
            </div>
          </td>
          <td class="mono muted">{{ h.path }}</td>
          <td class="mono muted">{{ fmtTime(h.added_at) }}</td>
          <td>
            <StateChip
              v-if="homeState(h) !== 'INCLUDED'"
              :token="homeState(h)"
            />
            <span v-else class="muted">включён</span>
          </td>
          <td class="mono muted">
            {{ schemaLabel(h.schema_version) }}
            <div v-if="h.version_note" class="muted" style="font-size: 12px">
              {{ h.version_note }}
            </div>
            <div v-if="homeIssue(h)" class="muted" style="font-size: 12px">
              <span class="code">{{ homeIssue(h)!.code }}</span
              >{{ homeIssueText(homeIssue(h)) }}
            </div>
          </td>
          <td style="text-align: right; white-space: nowrap">
            <button v-if="!h.active" :disabled="!canActivate(h)" @click="activate(h)">
              Активировать
            </button>
            <!-- Ratified OQ-HOME-8: the ACTIVE home can be switched off too
                 (the platform hands the marker to the next enabled record or
                 leaves the honest "no active home" state). -->
            <button style="margin-left: 6px" @click="toggle(h)">
              {{ h.enabled ? "Выключить" : "Включить" }}
            </button>
          </td>
        </tr>
      </tbody>
    </table>
  </section>
</template>
