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

/// Планировщик кадров и часы — швы для проверки без таймингов браузера.
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

/// Допуск на округление: браузер отдаёт scrollTop целым числом пикселей,
/// поэтому «попали точно в место» — это расхождение не больше одного пикселя.
const SETTLE_TOLERANCE_PX = 1;

/// Жесты, которыми человек управляет прокруткой сам: его место важнее
/// отложенного восстановления.
const USER_SCROLL_INTENTS = ["wheel", "touchstart", "touchmove", "keydown"];

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

/// Запомнить место экрана. Отрицательное значение — не место, а верх.
function remember(screen: string, top: number): void {
  offsets.set(screen, Math.max(0, top));
}

/// Насколько всего может уехать прокрутка: содержимое минус высота окна.
function offsetLimit(host: ScrollHost): number {
  return Math.max(0, host.scrollHeight - host.clientHeight);
}

/// Окно стоит ровно там, куда его просили (с точностью до округления).
function reached(host: ScrollHost, want: number): boolean {
  return Math.abs(host.scrollTop - want) <= SETTLE_TOLERANCE_PX;
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

/// Восстановление места одного экрана: помнит, какой экран сейчас открыт и
/// идёт ли ещё отложенное восстановление (его промежуточные движения не
/// записываются как место человека).
class ScreenRestorer {
  private activeKey: string | null = null;
  private cancelFrame: (() => void) | null = null;
  private restoring = false;
  private startedAt = 0;
  private readonly host: ScrollHost;
  private readonly frame: (step: () => void) => () => void;
  private readonly now: () => number;
  private readonly budget: number;

  constructor(
    host: ScrollHost,
    frame: (step: () => void) => () => void,
    now: () => number,
    budget: number,
  ) {
    this.host = host;
    this.frame = frame;
    this.now = now;
    this.budget = budget;
  }

  /// Какой экран считается открытым (для первого запуска окна).
  get active(): string | null {
    return this.activeKey;
  }

  /// Движение прокрутки: пока идёт восстановление, оно промежуточное и в
  /// память не попадает.
  observe(top: number): void {
    if (this.restoring || !this.activeKey) return;
    remember(this.activeKey, top);
  }

  /// Уход с экрана: запоминаем ровно то, что видит человек. Маршрутизатор
  /// зовёт afterEach до перерисовки экрана (vue-router: finalizeNavigation →
  /// triggerAfterEach одним блоком), поэтому контейнер ещё держит место
  /// уходящего экрана. Прерванное восстановление оставляет прежнее место.
  leave(screen: string, top: number): void {
    if (!this.restoring) remember(screen, top);
  }

  /// Человек сам взялся крутить — его место важнее отложенного восстановления.
  stopOnUserIntent(): void {
    if (this.restoring) this.stop();
  }

  stop(): void {
    this.cancelFrame?.();
    this.cancelFrame = null;
    this.restoring = false;
  }

  /// Экран переключается на место памяти ДО любых движений: чужая позиция
  /// не должна попасть в слот нового экрана.
  restore(key: string): void {
    this.stop();
    this.activeKey = key;
    const want = offsets.get(key) ?? 0;
    if (!want) {
      // Нечего помнить — экран открывается сверху, и это его место.
      this.settle(key, 0);
      return;
    }
    this.startedAt = this.now();
    this.restoring = true;
    this.schedule(key, want);
  }

  /// Восстановление закончилось: экран стоит там, где стоит сейчас.
  private settle(key: string, top: number): void {
    this.host.scrollTop = top;
    remember(key, this.host.scrollTop);
    this.stop();
  }

  /// Один кадр: применить место, как только содержимое доросло до него;
  /// иначе ждать (и сдаться на верх, когда срок вышел).
  private step(key: string, want: number): void {
    this.cancelFrame = null;
    if (!this.restoring || this.activeKey !== key) return;
    const fits = want <= offsetLimit(this.host);
    if (fits) {
      this.host.scrollTop = want;
      if (reached(this.host, want)) {
        this.stop();
        return;
      }
    }
    if (this.now() - this.startedAt >= this.budget) {
      // Место мимо содержимого (экран стал короче или не дорос) — сверху.
      this.settle(key, fits ? want : 0);
      return;
    }
    this.schedule(key, want);
  }

  private schedule(key: string, want: number): void {
    this.cancelFrame = this.frame(() => this.step(key, want));
  }
}

/// Подключить память к прокручиваемому контейнеру окна и маршрутизатору.
/// Возвращает отписку — окно снимает её при размонтировании корня.
export function installScrollMemory(
  host: ScrollHost,
  router: Router,
  options: ScrollMemoryOptions = {},
): () => void {
  const restorer = new ScreenRestorer(
    host,
    options.frame ?? defaultFrame,
    options.now ?? defaultNow,
    options.budgetMs ?? RESTORE_BUDGET_MS,
  );

  const onScroll: EventListener = () => restorer.observe(host.scrollTop);
  const onUserScroll: EventListener = () => restorer.stopOnUserIntent();
  host.addEventListener("scroll", onScroll);
  for (const type of USER_SCROLL_INTENTS) host.addEventListener(type, onUserScroll);

  const offRoute = router.afterEach((to, from, failure) => {
    // Переход не состоялся (страж отменил) — экран тот же, память не трогаем.
    if (failure) return;
    restorer.leave(from.path, host.scrollTop);
    restorer.restore(to.path);
  });

  // Первый экран: `afterEach` мог пройти до установки памяти.
  void router.isReady().then(() => {
    if (restorer.active === null) restorer.restore(router.currentRoute.value.path);
  });

  return () => {
    restorer.stop();
    host.removeEventListener("scroll", onScroll);
    for (const type of USER_SCROLL_INTENTS) host.removeEventListener(type, onUserScroll);
    offRoute();
  };
}

/// Подключить память к контейнеру, который оболочка окна помечает в разметке
/// (`data-scroll-host`). Так запуск окна и проверка пользуются одним путём:
/// поиск контейнера не дублируется в коде и в тесте строкой.
export function installScrollMemoryInDocument(
  router: Router,
  options: ScrollMemoryOptions = {},
): (() => void) | null {
  const host = document.querySelector<HTMLElement>("[data-scroll-host]");
  return host ? installScrollMemory(host, router, options) : null;
}
