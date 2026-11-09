// Связь страницы с домом, к которому её подключили (task 01a1bf78,
// REQ-QRAPP-05/06).
//
// Страница с телефона (браузер) не имеет нативной половины: отзыв доступа
// замечает ОДИН опрос `GET /pair` на всё приложение — дешёвый вопрос, ответ на
// который машина уже не присылает сама после «отключить» (REQ-QRAPP-05
// constraint). В окне приложения этот сторож не нужен: там отзыв замечает Rust
// и сам меняет экран, поэтому таймер поднимается ТОЛЬКО у страницы без моста.
//
// Почему молчание адреса — «доступ закрыт», а не «не та сеть»: страница
// загружена С САМОЙ машины, значит сеть была та; если адрес замолчал, закрылась
// дверь (REQ-QRAPP-06 constraint — сравнение с текущей сетью делает нативная
// половина, у неё есть список интерфейсов телефона).
import { ref } from "vue";
import { pageWatchesPairing } from "./api";
import { PAIR_REFUSALS, type PairReason, type PairRefusalView } from "./pairState";

/// Цикл проверки. Пять секунд — потолок из приёмки (REQ-QRAPP-05 AC1); тот же
/// цикл у нативной половины (`lan::pair::WATCH_POLL`), чтобы телефон не вёл
/// два разных счёта.
export const PAIR_POLL_MS = 3000;

/// Факт отказа: `null` — связь жива. Пока он не выставлен, ничего не меняется:
/// данные, уже показанные на экране, не стираются молча (REQ-QRAPP-06 AC5).
export const pairFailure = ref<(PairRefusalView & { code: PairReason }) | null>(null);

export type PairAnswer = "paired" | "refused" | "silent";

/// Один вопрос машине. `refused` — она ответила и отказала (401/403),
/// `silent` — не ответила вовсе.
export async function probePairing(fetchImpl: typeof fetch = fetch): Promise<PairAnswer> {
  try {
    const response = await fetchImpl("./pair", { headers: { accept: "application/json" } });
    return response.ok ? "paired" : "refused";
  } catch {
    return "silent";
  }
}

/// Во что превращается ответ. `null` — доступа не отзывали.
export function pairReasonForAnswer(answer: PairAnswer): PairReason | null {
  switch (answer) {
    case "paired":
      return null;
    case "refused":
    case "silent":
      return "access_closed";
  }
}

/// Один опрос; при отказе факт публикуется и дальше не опрашивается — по
/// протухшему коду запросов больше нет (REQ-QRAPP-05 AC3).
export async function checkPairing(fetchImpl: typeof fetch = fetch): Promise<void> {
  if (pairFailure.value) return;
  const reason = pairReasonForAnswer(await probePairing(fetchImpl));
  if (reason) pairFailure.value = { code: reason, ...PAIR_REFUSALS[reason] };
}

let timer: ReturnType<typeof setInterval> | undefined;
let subscribers = 0;

/// Подписка приложения: первый подписчик запускает единственный таймер,
/// последний его гасит. Вне страницы-на-телефоне не делает ничего.
export function subscribePairing(): () => void {
  if (!pageWatchesPairing) return () => {};
  subscribers += 1;
  if (subscribers === 1) {
    void checkPairing();
    timer = setInterval(() => void checkPairing(), PAIR_POLL_MS);
  }
  return () => {
    subscribers -= 1;
    if (subscribers === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

/// Тестовый шов: сброс синглтона между сценариями.
export function resetPairingForTests() {
  if (timer !== undefined) clearInterval(timer);
  timer = undefined;
  subscribers = 0;
  pairFailure.value = null;
}
