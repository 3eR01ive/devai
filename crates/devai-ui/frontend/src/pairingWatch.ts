// Тёплый скан (task 01a1bf78, quality return R1): приложение уже открыто, и
// окно показывает СВОЮ страницу — сканирование меняет только фрагмент адреса.
// Это same-document переход: `onMounted` корневого App.vue второй раз не
// срабатывает, поэтому ОДИН обмен с машиной надо запускать ещё и по смене
// адреса. Холодный старт и смена дома с чужого origin работают как раньше —
// там документ загружается заново.
//
// Здесь же ручной проход «продолжить»: если автоматический обмен не случился
// (страница не перезагрузилась, адрес не изменился, мост ответил ошибкой),
// человек не остаётся на «подключаюсь к дому…» без выхода. Слова и адреса
// этот модуль не придумывает: задача приходит значением из Rust.
import { runPairing, type PairBridge, type PairTask } from "./pairingNative";

/// Фрагмент адреса, которым Rust поднимает экран «скан принят»
/// (`lan::pair::connect_url`). Хвост `scan=<отпечаток>` делает адрес разным
/// для каждого приглашения — иначе повторный скан не отличался бы от текущего
/// и браузер не прислал бы `hashchange`.
export const CONNECT_ROUTE = "#/pair?step=connect";

/// Сколько человек видит «подключаюсь…», прежде чем экран честно скажет, что
/// машина не ответила. С запасом над бюджетом вопроса (`PROBE_TIMEOUT` 1.5 с),
/// чтобы медленная сеть не читалась как поломка.
export const PAIR_CONNECT_TIMEOUT_MS = 8000;

/// Экран сейчас — принятый скан (а не отказ и не обычный раздел окна).
export function isConnectHash(hash: string): boolean {
  return hash.startsWith(CONNECT_ROUTE);
}

/// Один проход для текущего адреса: `null` — экран не про скан, спрашивать
/// нечего и ходить никуда не нужно.
export async function pairThisScreen(
  hash: string,
  bridge: PairBridge,
): Promise<PairTask | null> {
  if (!isConnectHash(hash)) return null;
  return runPairing(bridge);
}

/// Один проход за раз: повторная доставка того же приглашения или два быстрых
/// скана не должны вести два обмена параллельно и дважды дёргать окно.
export function pairingRunner(
  bridge: PairBridge,
  onDone: (task: PairTask) => void,
  onError: (error: unknown) => void,
): () => void {
  let busy = false;
  return () => {
    if (busy) return;
    busy = true;
    runPairing(bridge)
      .then(onDone, onError)
      .finally(() => {
        busy = false;
      });
  };
}

/// Окно, у которого есть адрес и подписка на его смену (в приложении —
/// настоящий `window`, в тесте — двойник).
export interface PairingWindow {
  readonly location: { readonly hash: string };
  addEventListener(type: "hashchange", listener: () => void): void;
  removeEventListener(type: "hashchange", listener: () => void): void;
}

/// Слушатель смены адреса внутри приложения: срабатывает только на экране
/// скана, поэтому обычная навигация по разделам обмен не запускает.
export function watchScans(host: PairingWindow, run: () => void): () => void {
  const listener = () => {
    if (isConnectHash(host.location.hash)) run();
  };
  host.addEventListener("hashchange", listener);
  return () => host.removeEventListener("hashchange", listener);
}

// Ручной проход «продолжить»: экран приглашения просит прогон, корень окна
// его выполняет. Экран не знает ни про Tauri, ни про сеть — и в тестах
// остаётся обычной разметкой.
let runner: (() => void) | null = null;

export function setPairingRunner(run: (() => void) | null): void {
  runner = run;
}

export function requestPairingRun(): void {
  runner?.();
}

/// Тестовый шов: между сценариями не остаётся чужого прогона.
export function resetPairingWatchForTests(): void {
  runner = null;
}
