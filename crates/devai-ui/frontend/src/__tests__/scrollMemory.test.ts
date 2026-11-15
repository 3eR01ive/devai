// Память прокрутки по экранам (REQ-SCROLL-01…04): место списка задач не
// переезжает в открытую карточку, каждая задача помнит своё место, экраны
// независимы, а при неопределённости экран открывается сверху.
//
// jsdom раскладки не считает, поэтому высоты содержимого и окна задаются у
// подставного контейнера явно, а кадры восстановления прогоняются вручную —
// проверка не зависит от таймингов браузера.
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeEach, describe, expect, it } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { installScrollMemory, rememberedOffset, resetScrollMemory } from "../scrollMemory";

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const VIEWPORT = 400;

/// Подставной прокручиваемый контейнер: своя высота содержимого и место.
function scrollHost(contentHeight: number) {
  const el = document.createElement("div");
  let height = contentHeight;
  let top = 0;
  Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => height });
  Object.defineProperty(el, "clientHeight", { configurable: true, get: () => VIEWPORT });
  Object.defineProperty(el, "scrollTop", {
    configurable: true,
    get: () => top,
    set: (v: number) => {
      top = Math.max(0, Math.round(v));
    },
  });
  return {
    el,
    /// Содержимое экрана дорисовалось до такой высоты.
    setContent(h: number) {
      height = h;
    },
    /// Человек промотал экран: место и событие прокрутки, как в браузере.
    scrollTo(v: number) {
      top = v;
      el.dispatchEvent(new Event("scroll"));
    },
    top: () => top,
  };
}

/// Кадры восстановления: копим и прогоняем по шагу — тест сам решает, когда.
function frameQueue() {
  const pending = new Set<() => void>();
  return {
    frame: (step: () => void) => {
      pending.add(step);
      return () => pending.delete(step);
    },
    run() {
      const steps = [...pending];
      pending.clear();
      for (const step of steps) step();
    },
    reachable: () => pending.size,
  };
}

const Blank = { template: "<div />" };

function makeRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/projects", component: Blank },
      { path: "/models", component: Blank },
      { path: "/proto/tasks", component: Blank },
      { path: "/proto/task/:id", component: Blank },
    ],
  });
}

/// Установленная память + ручные кадры и часы.
function harness(contentHeight = 5000) {
  const host = scrollHost(contentHeight);
  const router = makeRouter();
  const frames = frameQueue();
  let clock = 0;
  const dispose = installScrollMemory(host.el, router, {
    frame: frames.frame,
    now: () => clock,
    budgetMs: 100,
  });
  return {
    host,
    router,
    frames,
    advance(ms: number) {
      clock += ms;
    },
    dispose,
  };
}

beforeEach(() => resetScrollMemory());

describe("место экрана помнится само", () => {
  it("список, промотанный вниз, не переносит место в открытую задачу", async () => {
    const { host, router, frames, dispose } = harness();
    await router.push("/proto/tasks");
    frames.run();
    host.scrollTo(1200);

    await router.push("/proto/task/a");
    frames.run();
    // Карточка открывается сверху, а не на высоте списка.
    expect(host.top()).toBe(0);

    await router.push("/proto/tasks");
    frames.run();
    // Вернувшись, человек видит список там, где его оставил.
    expect(host.top()).toBe(1200);
    dispose();
  });

  it("у каждой задачи своя позиция в карточке", async () => {
    const { host, router, frames, dispose } = harness();
    await router.push("/proto/tasks");
    frames.run();

    await router.push("/proto/task/a");
    frames.run();
    host.scrollTo(900);

    // Уходим в список и открываем ДРУГУЮ задачу: чужая позиция не наследуется.
    await router.push("/proto/tasks");
    frames.run();
    await router.push("/proto/task/b");
    frames.run();
    expect(host.top()).toBe(0);

    // Возврат в первую задачу — на её собственное место.
    await router.push("/proto/task/a");
    frames.run();
    expect(host.top()).toBe(900);
    dispose();
  });

  it("экран открывается сверху, если его ещё не открывали", async () => {
    const { host, router, frames, dispose } = harness();
    await router.push("/proto/tasks");
    frames.run();
    host.scrollTo(2000);

    await router.push("/projects");
    frames.run();
    expect(host.top()).toBe(0);
    expect(rememberedOffset("/projects")).toBe(0);
    dispose();
  });
});

describe("место мимо содержимого — сверху", () => {
  it("экран стал короче: запомненное место не оставляет человека в середине", async () => {
    const { host, router, frames, advance, dispose } = harness();
    await router.push("/proto/task/a");
    frames.run();
    host.scrollTo(4000);
    expect(rememberedOffset("/proto/task/a")).toBe(4000);

    await router.push("/proto/tasks");
    frames.run();
    // Задача стала короче: 900px содержимого при окне 400 (низ — 500).
    host.setContent(900);
    await router.push("/proto/task/a");
    frames.run();
    // Первый кадр: место ещё не помещается — ждём роста, не прыгаем наугад.
    expect(host.top()).toBe(0);
    frames.run();
    expect(host.top()).toBe(0);
    // Срок ожидания вышел — экран открыт сверху, ошибки нет.
    advance(120);
    frames.run();
    expect(host.top()).toBe(0);
    expect(rememberedOffset("/proto/task/a")).toBe(0);
    dispose();
  });

  it("содержимое пришло асинхронно: место восстанавливается, когда экран дорос", async () => {
    const { host, router, frames, dispose } = harness(600);
    await router.push("/proto/task/a");
    frames.run();
    host.scrollTo(3000);
    await router.push("/proto/tasks");
    frames.run();

    // Возврат: экран ещё пустой, запомненное место пока не помещается.
    await router.push("/proto/task/a");
    frames.run();
    expect(host.top()).toBe(0);
    host.setContent(5000);
    frames.run();
    expect(host.top()).toBe(3000);
    dispose();
  });
});

describe("прокрутка одного экрана не влияет на другой", () => {
  it("проекты, модели и задачи помнят каждый своё место", async () => {
    const { host, router, frames, dispose } = harness();
    await router.push("/projects");
    frames.run();
    host.scrollTo(300);

    await router.push("/models");
    frames.run();
    host.scrollTo(700);

    await router.push("/proto/tasks");
    frames.run();
    expect(host.top()).toBe(0);
    host.scrollTo(1100);

    await router.push("/models");
    frames.run();
    expect(host.top()).toBe(700);

    await router.push("/projects");
    frames.run();
    expect(host.top()).toBe(300);

    await router.push("/proto/tasks");
    frames.run();
    expect(host.top()).toBe(1100);
    dispose();
  });

  it("своя прокрутка внутри карточки не меняет место экрана", async () => {
    const { host, router, frames, dispose } = harness();
    await router.push("/proto/task/a");
    frames.run();

    // Блок кода со своей прокруткой: событие прокрутки вложенного элемента
    // не всплывает до контейнера экрана (REQ-SCROLL-04).
    const inner = document.createElement("pre");
    host.el.appendChild(inner);
    inner.scrollTop = 800;
    inner.dispatchEvent(new Event("scroll"));

    expect(host.top()).toBe(0);
    expect(rememberedOffset("/proto/task/a")).toBe(0);
    dispose();
  });
});

describe("окно подключает память к своему контейнеру", () => {
  it("оболочка помечает контейнер, а запуск окна ставит на него память", () => {
    const app = readFileSync(join(SRC, "App.vue"), "utf8");
    const main = readFileSync(join(SRC, "main.ts"), "utf8");
    // Разметка оболочки: тот самый единственный прокручиваемый контейнер.
    expect(app).toContain('<main class="content" data-scroll-host>');
    // Запуск окна: находит его по этой метке и подключает память.
    expect(main).toContain('querySelector<HTMLElement>("[data-scroll-host]")');
    expect(main).toContain("installScrollMemory(scrollHost, router)");
  });
});
