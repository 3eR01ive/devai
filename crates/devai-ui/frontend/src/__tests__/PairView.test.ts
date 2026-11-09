// Экран отказа в приглашении (task 01a1bf78, REQ-QRAPP-06): причина приходит
// кодом из адреса, а слова — из единственной декларации pairState. Проверяем,
// что три состояния называют себя РАЗНЫМИ словами и что экран не выдумывает
// фразу для неизвестного кода.
import { mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { describe, expect, it } from "vitest";
import { PAIR_REASONS } from "../pairState";
import PairView from "../views/PairView.vue";

const Blank = { template: "<div/>" };

async function show(reason?: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", component: Blank },
      { path: "/pair", component: PairView },
    ],
  });
  await router.push(reason ? `/pair?reason=${reason}` : "/pair");
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

  it("просроченное приглашение оставляет открытый дом и предлагает вернуться", async () => {
    const view = await show("invite_expired");
    expect(view.get("h2").text()).toBe("приглашение просрочено");
    expect(view.text()).toContain("Открытый дом не тронут");
    expect(view.find("a").exists()).toBe(true);
    view.unmount();
  });

  it("закрытый доступ не обещает вернуться: машина не отвечает", async () => {
    const view = await show("access_closed");
    expect(view.get("h2").text()).toBe("доступ закрыт");
    expect(view.find("a").exists()).toBe(false);
    view.unmount();
  });

  it("неизвестный код не превращается в выдуманную причину", async () => {
    const view = await show("something_else");
    expect(view.get("h2").text()).toBe("приглашение не принято");
    expect(view.find("[data-testid='pair-hint']").exists()).toBe(false);
    view.unmount();
  });
});
