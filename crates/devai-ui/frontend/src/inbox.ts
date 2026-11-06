// Единственный источник снимка секции ожидания (R3 QA-возврата): шапка
// (бейдж) и вкладка #/inbox читают ОДИН стор, ОДИН интервал опроса и не
// дублируют вычисление снимка. Раньше каждый из двух компонентов вёл
// собственный таймер и свой вызов `inbox_view` — батч-чтение считалось
// дважды на каждый тик. Здесь: один inflight-запрос (повторные вызовы
// присоединяются к нему), один интервал на всех подписчиков, последний
// снимок — общий ref. Кэша ожидания нет: значение живёт до следующего
// load и целиком приходит из published-read (REQ-GATE-07).
import { ref } from "vue";
import { api, toUiError } from "./api";
import { pollMs } from "./settings";
import type { UiError, WaitSnapshot } from "./types";

/// Последний полученный снимок секции — общий для всех потребителей.
export const waitSnapshot = ref<WaitSnapshot | null>(null);
/// Ошибка последнего load; снимок при этом сохраняется (статус-бар owns
/// свои факты, секция показывает старую правду до следующего успеха).
export const waitError = ref<UiError | null>(null);

let inflight: Promise<void> | null = null;
let timer: number | undefined;
let subscribers = 0;

/// Один запрос на всех: параллельные вызовы присоединяются к inflight.
export function refreshWait(): Promise<void> {
  if (inflight) return inflight;
  inflight = api
    .inboxView()
    .then((snap) => {
      waitSnapshot.value = snap;
      waitError.value = null;
    })
    .catch((e) => {
      waitError.value = toUiError(e);
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

function seatTimer(ms: number) {
  if (timer !== undefined) window.clearInterval(timer);
  timer = window.setInterval(() => void refreshWait(), ms);
}

/// Подписка компонента: первый подписчик запускает единственный интервал
/// и первый load; возвращённая функция отписывает (последний — глушит).
export function subscribeWait(): () => void {
  subscribers += 1;
  if (subscribers === 1) {
    void refreshWait();
    seatTimer(pollMs());
  }
  return () => {
    subscribers -= 1;
    if (subscribers === 0 && timer !== undefined) {
      window.clearInterval(timer);
      timer = undefined;
    }
  };
}

/// Смена интервала из UI-настройки: пересаживает тот же единственный
/// таймер (интервал — настройка потребителя, не платформенное правило).
export function setWaitInterval(ms: number) {
  if (timer !== undefined) seatTimer(ms);
}

/// Тестовый шов: сброс синглтона между сценариями.
export function resetWaitStoreForTests() {
  if (timer !== undefined) window.clearInterval(timer);
  timer = undefined;
  subscribers = 0;
  inflight = null;
  waitSnapshot.value = null;
  waitError.value = null;
}
