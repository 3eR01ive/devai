// Снимок «что идёт прямо сейчас» для шапки нового списка: один стор, один
// интервал, одно чтение `task.active` на всех подписчиков (та же дисциплина,
// что у секции ожидания в inbox.ts).
import { ref } from "vue";
import { api, toUiError } from "./api";
import { pollMs } from "./settings";
import type { ActiveSnapshot, UiError } from "./types";

export const activeSnapshot = ref<ActiveSnapshot | null>(null);
export const activeError = ref<UiError | null>(null);

let inflight: Promise<void> | null = null;
let timer: number | undefined;
let subscribers = 0;

export function refreshActive(): Promise<void> {
  if (inflight) return inflight;
  inflight = api
    .taskActive()
    .then((snap) => {
      activeSnapshot.value = snap;
      activeError.value = null;
    })
    .catch((e) => {
      activeError.value = toUiError(e);
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

export function subscribeActive(): () => void {
  subscribers += 1;
  if (subscribers === 1) {
    void refreshActive();
    timer = window.setInterval(() => void refreshActive(), pollMs());
  }
  return () => {
    subscribers -= 1;
    if (subscribers === 0 && timer) {
      window.clearInterval(timer);
      timer = undefined;
    }
  };
}

export function resetActiveStoreForTests(): void {
  activeSnapshot.value = null;
  activeError.value = null;
  inflight = null;
  subscribers = 0;
  if (timer) {
    window.clearInterval(timer);
    timer = undefined;
  }
}
