// Память прокрутки по экранам (REQ-SCROLL-01…04): место списка задач не
// переезжает в открытую карточку, каждая задача помнит своё место, экраны
// независимы, а при неопределённости экран открывается сверху.
//
// jsdom раскладки не считает, поэтому высоты содержимого и окна задаются у
// контейнера явно, а кадры восстановления прогоняются вручную — проверка не
// зависит от таймингов браузера. Два последних кейса идут дальше подставного
// объекта: монтируется настоящая оболочка окна и настоящий RouterView, так что
// порядок «маршрут сменился → DOM перерисован» проверяется на живой разметке.
import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h, nextTick } from "vue";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App.vue";
import { installScrollMemory, installScrollMemoryInDocument, rememberedOffset, resetScrollMemory } from "../scrollMemory";

const mocks = vi.hoisted(() => ({
  sessionInfo: vi.fn(),
  inboxView: vi.fn(),
  taskActive: vi.fn(),
}));

vi.mock("../api", () => ({
  api: {
    sessionInfo: mocks.sessionInfo,
    inboxView: mocks.inboxView,
    taskActive: mocks.taskActive,
  },
  toUiError: (e: unknown) => e,
  // Окно оператора: приглашение проводит своя страница, но в этом кейсе его нет.
  pageWatchesPairing: false,
  pairingShell: false,
}));

const VIEWPORT = 400;

/// Прокручиваемый контейнер без раскладки: своя высота содержимого и место.
function layout(el: HTMLElement, contentHeight: number) {
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

/// Подставной контейнер: тот же layout на отдельном элементе.
function scrollHost(contentHeight: number) {
  const el = document.createElement("div");
  return { el, ...layout(el, contentHeight) };
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

const Blank = { render: () => h("div") };

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

beforeEach(() => {
  resetScrollMemory();
  mocks.sessionInfo.mockResolvedValue({ home: "/home/test", actor: "tester", opened_at: "2026-11-15T10:00:00Z" });
  mocks.inboxView.mockResolvedValue({ badge: 0, tasks: [], stopped: [], notes: [] });
  mocks.taskActive.mockResolvedValue({ snapshot_at: "", rows: [] });
});

afterEach(() => {
  document.body.innerHTML = "";
});

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
  it("оболочка отдаёт единственный прокручиваемый контейнер, и память ставится на него", async () => {
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { stubs: { RouterLink: { template: "<a><slot /></a>" }, RouterView: { template: "<div />" } } },
    });
    await flushPromises();
    // Разметка оболочки: ровно один прокручиваемый контейнер, помеченный для
    // памяти. Метка проверяется на смонтированном окне, а не строкой шаблона.
    const root = wrapper.element as HTMLElement;
    const host = root.querySelector("[data-scroll-host]");
    expect(host).not.toBeNull();
    expect(root.querySelectorAll("[data-scroll-host]").length).toBe(1);

    const view = layout(host as HTMLElement, 5000);
    const router = makeRouter();
    const frames = frameQueue();
    const dispose = installScrollMemoryInDocument(router, {
      frame: frames.frame,
      now: () => 0,
      budgetMs: 100,
    });
    // Контейнер найден по метке — окно действительно получает память.
    expect(dispose).toBeTypeOf("function");

    await router.push("/proto/tasks");
    await nextTick();
    frames.run();
    view.scrollTo(1200);
    expect(rememberedOffset("/proto/tasks")).toBe(1200);

    dispose?.();
    wrapper.unmount();
  });

  it("без помеченного контейнера память не ставится и не падает", () => {
    expect(installScrollMemoryInDocument(makeRouter())).toBeNull();
  });
});

describe("на живой разметке: уход с экрана запоминается до перерисовки", () => {
  const screen = (cls: string) =>
    defineComponent({
      setup() {
        return () => h("section", { class: cls }, cls);
      },
    });

  /// Настоящая оболочка: тот же контейнер, но внутри живой RouterView —
  /// смена маршрута действительно заменяет DOM-содержимое.
  function mountShell() {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/", component: screen("list") },
        { path: "/proto/tasks", component: screen("list") },
        { path: "/proto/task/:id", component: screen("card") },
      ],
    });
    const Shell = defineComponent({
      setup() {
        return () => h("div", { class: "content", "data-scroll-host": "" }, [h(RouterView)]);
      },
    });
    const wrapper = mount(Shell, { global: { plugins: [router] }, attachTo: document.body });
    const view = layout(wrapper.element as HTMLElement, 5000);
    const frames = frameQueue();
    const dispose = installScrollMemoryInDocument(router, {
      frame: frames.frame,
      now: () => 0,
      budgetMs: 100,
    });
    return { wrapper, router, view, frames, dispose };
  }

  it("место списка снято со старого экрана, а карточка уже сверху", async () => {
    const { wrapper, router, view, frames, dispose } = mountShell();
    await router.push("/proto/tasks");
    await nextTick();
    frames.run();
    expect(wrapper.element.querySelector(".list")).not.toBeNull();

    view.scrollTo(1200);
    await router.push("/proto/task/a");
    await nextTick();
    frames.run();

    // DOM действительно сменился: в окне карточка, а не список.
    expect(wrapper.element.querySelector(".card")).not.toBeNull();
    expect(wrapper.element.querySelector(".list")).toBeNull();
    // Уходящий экран запомнил своё место ДО перерисовки (иначе здесь был бы 0).
    expect(rememberedOffset("/proto/tasks")).toBe(1200);
    expect(view.top()).toBe(0);

    // И возврат ставит живой список на прежнее место.
    await router.push("/proto/tasks");
    await nextTick();
    frames.run();
    expect(view.top()).toBe(1200);

    dispose?.();
    wrapper.unmount();
  });
});
