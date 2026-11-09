// Экран отказа в приглашении (task 01a1bf78, REQ-QRAPP-06): причина приходит
// кодом из адреса, а слова — из единственной декларации pairState. Проверяем,
// что три состояния называют себя РАЗНЫМИ словами, что экран не выдумывает
// фразу для неизвестного кода и что «вернуться» решает Rust (признак return=1),
// а не список причин на этой стороне.
import { mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PAIR_REASONS } from "../pairState";
import PairView from "../views/PairView.vue";

// Кнопка «продолжить» просит прогон у корня окна; здесь проверяется сама
// просьба, а не сетевой обмен (его ведёт pairingRunner/windowBridge).
const mocks = vi.hoisted(() => ({ continuePairing: vi.fn() }));

vi.mock("../pairingWatch", () => ({
  PAIR_CONNECT_TIMEOUT_MS: 8000,
  requestPairingRun: mocks.continuePairing,
}));

afterEach(() => {
  vi.useRealTimers();
  mocks.continuePairing.mockClear();
});

const Blank = { template: "<div/>" };

async function show(reason?: string, back = false, step?: string, scan?: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", component: Blank },
      { path: "/pair", component: PairView },
    ],
  });
  const query: string[] = [];
  if (reason) query.push(`reason=${reason}`);
  if (back) query.push("return=1");
  if (step) query.push(`step=${step}`);
  if (scan) query.push(`scan=${scan}`);
  await router.push(query.length ? `/pair?${query.join("&")}` : "/pair");
  await router.isReady();
  return mount(PairView, { global: { plugins: [router] } });
}

describe("отказ в приглашении", () => {
  it("каждая причина названа своими словами, а не общей ошибкой", async () => {
    const titles = new Set<string>();
    for (const reason of PAIR_REASONS) {
      const view = await show(reason);
      const hint = view.get("[data-testid='pair-hint']").text();
      expect(hint.length).toBeGreaterThan(0);
      titles.add(view.get("h2").text());
      view.unmount();
    }
    expect(titles.size).toBe(PAIR_REASONS.length);
    expect([...titles]).toEqual(
      expect.arrayContaining(["приглашение просрочено", "не та сеть", "доступ закрыт"]),
    );
  });

  it("отказ на открытом доме предлагает вернуться к нему", async () => {
    const view = await show("invite_expired", true);
    expect(view.get("h2").text()).toBe("приглашение просрочено");
    expect(view.text()).toContain("Открытый дом не тронут");
    expect(view.find("a").exists()).toBe(true);
    view.unmount();
  });

  it("без открытого дома возвращаться некуда, даже если причина та же", async () => {
    const view = await show("invite_expired");
    expect(view.find("a").exists()).toBe(false);
    expect(view.text()).not.toContain("Открытый дом не тронут");
    view.unmount();
  });

  it("чужая сеть на открытом доме тоже оставляет рабочую сессию (качество R2)", async () => {
    const view = await show("wrong_network", true);
    expect(view.get("h2").text()).toBe("не та сеть");
    expect(view.find("a").exists()).toBe(true);
    view.unmount();
  });

  it("во время проверки экран говорит, что скан принят, а не показывает чужой дом", async () => {
    const view = await show(undefined, false, "connect");
    expect(view.get("h2").text()).toBe("подключаюсь к дому…");
    expect(view.find("[data-testid='pair-connect']").exists()).toBe(true);
    // Никакой причины отказа и никакой «пустой ошибки» на этом экране нет.
    expect(view.find("[data-testid='pair-hint']").exists()).toBe(false);
    expect(view.find("a").exists()).toBe(false);
    view.unmount();
  });

  it("скан можно продолжить, а по таймауту экран честно говорит о молчании", async () => {
    vi.useFakeTimers();
    const view = await show(undefined, false, "connect");
    expect(view.find("[data-testid='pair-continue']").exists()).toBe(true);
    expect(view.find("[data-testid='pair-stalled']").exists()).toBe(false);

    // Машина не ответила — человек не остаётся на вечном «подключаюсь…».
    await vi.advanceTimersByTimeAsync(8000);
    expect(view.find("[data-testid='pair-stalled']").exists()).toBe(true);

    // «продолжить» просит новый проход и снова начинает ждать.
    await view.get("[data-testid='pair-continue']").trigger("click");
    expect(mocks.continuePairing).toHaveBeenCalledTimes(1);
    expect(view.find("[data-testid='pair-stalled']").exists()).toBe(false);
    view.unmount();
  });

  it("новый скан на том же экране начинает ожидание заново", async () => {
    vi.useFakeTimers();
    const view = await show(undefined, false, "connect", "first");
    await vi.advanceTimersByTimeAsync(8000);
    expect(view.find("[data-testid='pair-stalled']").exists()).toBe(true);

    // Второе приглашение — другой отпечаток в адресе: ожидание новое, а не
    // унаследованное «машина не ответила» от прошлого скана.
    await view.vm.$router.replace("/pair?step=connect&scan=second");
    await vi.advanceTimersByTimeAsync(0);
    expect(view.find("[data-testid='pair-stalled']").exists()).toBe(false);
    await vi.advanceTimersByTimeAsync(8000);
    expect(view.find("[data-testid='pair-stalled']").exists()).toBe(true);
    view.unmount();
  });

  it("неизвестный код не превращается в выдуманную причину", async () => {
    const view = await show("something_else");
    expect(view.get("h2").text()).toBe("приглашение не принято");
    expect(view.find("[data-testid='pair-hint']").exists()).toBe(false);
    view.unmount();
  });
});
