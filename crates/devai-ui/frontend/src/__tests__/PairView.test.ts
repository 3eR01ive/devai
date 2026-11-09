// Экран отказа в приглашении (task 01a1bf78, REQ-QRAPP-06): причина приходит
// кодом из адреса, а слова — из единственной декларации pairState. Проверяем,
// что три состояния называют себя РАЗНЫМИ словами, что экран не выдумывает
// фразу для неизвестного кода и что «вернуться» решает Rust (признак return=1),
// а не список причин на этой стороне.
import { mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { describe, expect, it } from "vitest";
import { PAIR_REASONS } from "../pairState";
import PairView from "../views/PairView.vue";

const Blank = { template: "<div/>" };

async function show(reason?: string, back = false, step?: string) {
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

  it("неизвестный код не превращается в выдуманную причину", async () => {
    const view = await show("something_else");
    expect(view.get("h2").text()).toBe("приглашение не принято");
    expect(view.find("[data-testid='pair-hint']").exists()).toBe(false);
    view.unmount();
  });
});
