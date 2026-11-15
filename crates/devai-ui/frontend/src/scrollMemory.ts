// Память прокрутки по экранам (REQ-SCROLL-01…04).
//
// У окна ОДИН прокручиваемый контейнер — `main.content` (styles.css:110-117), и
// прокрутка у него общая на все маршруты. Поэтому место, где человек оставил
// список задач, переезжало в открытую карточку: она показывалась с середины.
// Здесь каждый экран помнит своё место сам: ключ памяти — адрес экрана, у
// карточки задачи в адресе есть её id, поэтому у каждой задачи своя позиция.
//
// Правила:
//   • уходя с экрана — запоминаем, где человек его оставил;
//   • приходя на экран — восстанавливаем его собственное место;
//   • места нет, оно не определилось или указывает мимо содержимого (экран
//     стал короче) — экран открывается сверху;
//   • содержимое приходит асинхронно, поэтому восстановление ждёт, пока
//     высота дорастёт до запомненного места, но не дольше budgetMs.
//
// Собственные прокрутки ВНУТРИ экрана (блок кода в карточке задачи со своей
// прокруткой) остаются как есть: события прокрутки вложенных элементов до
// контейнера не всплывают, поэтому память экрана их не видит и не трогает.
import type { Router } from "vue-router";

/// Минимум от прокручиваемого элемента, который нужен памяти. Отдельный тип —
/// чтобы проверять поведение на подставном элементе без раскладки браузера.
export interface ScrollHost {
  scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
  addEventListener(type: string, listener: EventListener, options?: AddEventListenerOptions | boolean): void;
  removeEventListener(type: string, listener: EventListener, options?: EventListenerOptions | boolean): void;
}

export interface ScrollMemoryOptions {
  /// Планировщик кадров: по умолчанию requestAnimationFrame.
  frame?: (step: () => void) => () => void;
  /// Часы для срока ожидания: по умолчанию performance.now().
  now?: () => number;
  /// Сколько ждать, пока содержимое дорастёт до запомненного места.
  budgetMs?: number;
}

/// Сколько ждём растущее содержимое, прежде чем считать место потерянным.
/// Срок ограничен: лучше открыть сверху, чем оставить человека в середине.
const RESTORE_BUDGET_MS = 1200;

/// Место каждого экрана: ключ — адрес экрана. Память живёт в окне: после
/// перезапуска её нет, и это честный «не определить — в начало».
const offsets = new Map<string, number>();

/// Что помнится про экран (для проверок и отчёта).
export function rememberedOffset(screen: string): number | undefined {
  return offsets.get(screen);
}

/// Забыть всё: состояние одного окна не переносится на другое.
export function resetScrollMemory(): void {
  offsets.clear();
}

const defaultFrame = (step: () => void): (() => void) => {
  if (typeof requestAnimationFrame === "function") {
    const id = requestAnimationFrame(() => step());
    return () => cancelAnimationFrame(id);
  }
  const id: ReturnType<typeof setTimeout> = setTimeout(step, 16);
  return () => clearTimeout(id);
};

const defaultNow = (): number =>
  typeof performance !== "undefined" ? performance.now() : Date.now();

/// Подключить память к прокручиваемому контейнеру окна и маршрутизатору.
/// Возвращает отписку — окно снимает её при размонтировании корня.
export function installScrollMemory(
  host: ScrollHost,
  router: Router,
  options: ScrollMemoryOptions = {},
): () => void {
  const frame = options.frame ?? defaultFrame;
  const now = options.now ?? defaultNow;
  const budget = options.budgetMs ?? RESTORE_BUDGET_MS;

  let activeKey: string | null = null;
  let cancelFrame: (() => void) | null = null;
  let restoring = false;

  function stopRestore(): void {
    cancelFrame?.();
    cancelFrame = null;
    restoring = false;
  }

  function remember(screen: string, top: number): void {
    offsets.set(screen, Math.max(0, top));
  }

  // Живое место экрана: пишем каждое движение, чтобы возврат был точным.
  // Пока идёт восстановление, движения промежуточные — их не запоминаем.
  const onScroll: EventListener = () => {
    if (restoring || !activeKey) return;
    remember(activeKey, host.scrollTop);
  };

  // Человек сам взялся крутить — его место важнее отложенного восстановления.
  const onUserScroll: EventListener = () => {
    if (restoring) stopRestore();
  };

  /// Восстановить место экрана `key` (или открыть его сверху).
  function restore(key: string): void {
    stopRestore();
    activeKey = key;
    // Экран переключается на место памяти ДО любых движений: чужая позиция
    // не должна попасть в слот нового экрана.
    const want = offsets.get(key) ?? 0;
    if (!want) {
      host.scrollTop = 0;
      remember(key, 0);
      return;
    }
    const startedAt = now();
    restoring = true;
    const step = () => {
      cancelFrame = null;
      if (!restoring || activeKey !== key) return;
      const max = Math.max(0, host.scrollHeight - host.clientHeight);
      if (want <= max) {
        host.scrollTop = want;
        if (Math.abs(host.scrollTop - want) <= 1) {
          stopRestore();
          return;
        }
      }
      if (now() - startedAt >= budget) {
        // Место мимо содержимого (экран стал короче или не дорос) — сверху.
        host.scrollTop = want <= max ? want : 0;
        // Экран открыт сверху — это и есть его место, старое забыто.
        remember(key, host.scrollTop);
        stopRestore();
        return;
      }
      cancelFrame = frame(step);
    };
    cancelFrame = frame(step);
  }

  host.addEventListener("scroll", onScroll);
  const userIntents = ["wheel", "touchstart", "touchmove", "keydown"];
  for (const type of userIntents) host.addEventListener(type, onUserScroll);

  const offRoute = router.afterEach((to, from, failure) => {
    // Переход не состоялся (страж отменил) — экран тот же, память не трогаем.
    if (failure) return;
    // Уходящий экран: запоминаем ровно то, что видит человек. Маршрутизатор
    // зовёт afterEach до перерисовки экрана (vue-router: finalizeNavigation →
    // triggerAfterEach одним блоком), поэтому контейнер ещё держит место
    // уходящего экрана. Пока шло восстановление, scrollTop промежуточный —
    // оставляем прежнее место.
    if (!restoring) remember(from.path, host.scrollTop);
    restore(to.path);
  });

  // Первый экран: `afterEach` мог пройти до установки памяти.
  void router.isReady().then(() => {
    if (activeKey === null) restore(router.currentRoute.value.path);
  });

  return () => {
    stopRestore();
    host.removeEventListener("scroll", onScroll);
    for (const type of userIntents) host.removeEventListener(type, onUserScroll);
    offRoute();
  };
}
