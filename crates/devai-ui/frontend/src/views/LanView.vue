<script setup lang="ts">
// Раздел «Доступ с телефона» (task 01a1be98, REQ-LAN-01…09).
//
// Одна кнопка включает и выключает доступ; состояние — ФАКТ сервера, а не
// намерение. Разбор состояния живёт в ../lanState (единая запись по всем
// объявленным состояниям); здесь только булевы факты и данные.
// Пока приглашение не использовано, здесь лежит QR: телефон в той же сети
// сканирует его камерой и открывает ТУ ЖЕ страницу с данными этой машины.
// Панель работает и на самом телефоне — те же команды, та же дверь.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { api, toUiError } from "../api";
import { lanStateView } from "../lanState";
import { fmtTime } from "../settings";
import type { LanStatus, UiError } from "../types";

const status = ref<LanStatus | null>(null);
const error = ref<UiError | null>(null);
const busy = ref(false);
let timer: ReturnType<typeof setInterval> | undefined;

const facts = computed(() => lanStateView(status.value));

async function load() {
  try {
    status.value = await api.lanStatus();
    error.value = null;
  } catch (e) {
    // Связь с машиной оборвалась (страница осталась открытой, а доступ
    // выключили или сменилась сеть) — это и есть честный ответ (REQ-LAN-08).
    error.value = toUiError(e);
  }
}

async function act(run: () => Promise<LanStatus>) {
  busy.value = true;
  error.value = null;
  try {
    status.value = await run();
  } catch (e) {
    error.value = toUiError(e);
  } finally {
    busy.value = false;
  }
}

const enable = () => act(() => api.lanEnable());
const disable = () => act(() => api.lanDisable());
const newInvite = () => act(() => api.lanNewInvite());
const disconnect = (id: string) => act(() => api.lanDisconnect(id));

onMounted(() => {
  void load();
  // Короткий опрос: подключение телефона и его пропажа видны сразу, а не
  // после перезапуска окна (REQ-LAN-09).
  timer = setInterval(() => void load(), 2000);
});
onUnmounted(() => {
  if (timer) clearInterval(timer);
});
</script>

<template>
  <section>
    <div class="rowflex" style="margin-bottom: 12px">
      <h2 style="margin: 0">Доступ с телефона</h2>
      <span class="lan-state" :class="facts.state" data-testid="lan-state">{{ facts.label }}</span>
      <div class="spacer" />
      <button v-if="facts.canEnable" :disabled="busy" @click="enable">Включить доступ</button>
      <button v-else-if="facts.state" :disabled="busy" @click="disable">Выключить доступ</button>
    </div>

    <div v-if="error" class="errorbox">
      <span class="code">{{ error.code }}</span>{{ error.message }}
    </div>

    <!-- Флаг «был включён» не сохранился: следующий запуск об этом не знает
         (quality return R3) — это видно, а не проглочено. -->
    <div v-if="status?.persist_error" class="errorbox" data-testid="lan-persist-error">
      <span class="code">not_saved</span>Не удалось сохранить настройку: {{ status.persist_error }}
    </div>

    <!-- Выключен: честная причина, без «включён» при недоступном доступе. -->
    <div v-if="status && facts.canEnable" class="panel" data-testid="lan-off">
      <div v-if="status.reason" class="muted" data-testid="lan-reason">
        <span class="mono">{{ status.reason_code }}</span> · {{ status.reason }}
      </div>
      <div v-else class="muted">
        Доступ выключен. Включите его, чтобы телефон в этой же сети открыл интерфейс
        с задачами, гейтами и домами этой машины.
      </div>
      <div class="muted">
        Слушает только адрес машины в домашней сети; облака, туннелей и проброса портов нет.
      </div>
    </div>

    <!-- Включён и ждёт: QR + одноразовый код. -->
    <div v-if="facts.showInvitation" class="panel">
      <div class="rowflex">
        <b>{{ facts.qrFailed ? "QR не построился" : "Отсканируйте камерой телефона" }}</b>
        <div class="spacer" />
        <button :disabled="busy" @click="newInvite">Новое приглашение</button>
      </div>
      <!-- QR не построился — это отдельный факт, НЕ «код использован»:
           адрес и код показаны текстом, телефон открывает страницу вручную. -->
      <div v-if="facts.qrFailed" class="errorbox" data-testid="lan-qr-error">
        {{ status?.qr_error }}. Откройте на телефоне: {{ status?.url }}
      </div>
      <div class="lan-qr">
        <!-- SVG строит сервер (crate qrcode): в QR только адрес машины и
             одноразовый токен, ничего внешнего. -->
        <div v-if="!facts.qrFailed" class="lan-qr-image" data-testid="lan-qr" v-html="status?.qr_svg" />
        <div class="lan-invite">
          <div class="mono" data-testid="lan-address">{{ status?.address }}</div>
          <div class="mono muted lan-token" data-testid="lan-token">{{ status?.token }}</div>
          <div class="muted">
            Код одноразовый: после первого подключения он гаснет, а телефон
            остаётся подключённым. «Новое приглашение» выпускает код для
            следующего устройства, не разрывая уже подключённые.
          </div>
        </div>
      </div>
    </div>

    <!-- Включён, но действующего приглашения нет (код уже использован). -->
    <div v-else-if="facts.showInviteMissing" class="panel muted" data-testid="lan-no-invite">
      Приглашение использовано. «Новое приглашение» выпустит код для следующего
      устройства; уже подключённые телефоны при этом не рвутся.
    </div>

    <!-- Кто подключён (ratified Q2: устройства могут быть разными). -->
    <div v-if="facts.showDevices" class="panel">
      <div class="rowflex">
        <b>Подключённые устройства</b>
        <div class="spacer" />
        <span class="mono muted">{{ status?.address }}</span>
        <button :disabled="busy" @click="newInvite">Ещё приглашение</button>
      </div>
      <table class="data">
        <thead>
          <tr>
            <th>Устройство</th>
            <th>Адрес</th>
            <th>Подключено</th>
            <th>Последний контакт</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="d in status?.devices ?? []" :key="d.id">
            <td data-label="Устройство" data-mobile="title">
              <b>{{ d.name }}</b>
              <span v-if="!d.live" class="mono muted"> · не отвечает</span>
            </td>
            <td class="mono" data-label="Адрес">{{ d.addr }}</td>
            <td class="mono" data-label="Подключено" data-mobile="head">{{ fmtTime(d.connected_at) }}</td>
            <td class="mono" data-label="Последний контакт">{{ fmtTime(d.last_seen) }}</td>
            <td data-label="Действия"><button :disabled="busy" @click="disconnect(d.id)">Отключить</button></td>
          </tr>
        </tbody>
      </table>
      <div class="muted">
        Действия с телефона идут тем же путём, что из окна и командной строки,
        и помечаются в истории источником «телефон: имя и адрес».
      </div>
    </div>
  </section>
</template>

<style scoped>
.lan-state {
  text-transform: uppercase;
  letter-spacing: 0.06em;
  font-size: 11px;
  border: 1px solid var(--panel-border);
  border-radius: 999px;
  padding: 1px 9px;
}
.lan-state.off {
  color: var(--slate);
}
.lan-state.awaiting {
  color: var(--orange);
  border-color: var(--orange);
}
.lan-state.connected {
  color: var(--green, #7fbf7f);
  border-color: var(--green, #7fbf7f);
}
.lan-qr {
  display: flex;
  flex-wrap: wrap;
  gap: 18px;
  align-items: flex-start;
  margin-top: 10px;
}
.lan-qr-image :deep(svg) {
  display: block;
  max-width: 100%;
  height: auto;
  background: #fff;
  border-radius: 6px;
  padding: 4px;
}
.lan-invite {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-width: 520px;
  min-width: 0;
}
/* Телефон: QR и приглашение — друг под другом, а не в две колонки
   (REQ-RESP-01). Планшет и десктоп сохраняют текущий ряд. */
@media (max-width: 767px) {
  .lan-qr {
    flex-direction: column;
    align-items: stretch;
  }
}
.lan-token {
  word-break: break-all;
  font-size: 12px;
}
</style>
