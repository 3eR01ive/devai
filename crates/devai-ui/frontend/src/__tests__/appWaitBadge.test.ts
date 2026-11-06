// Один снимок секции на приложение. Шапка (бейдж) и
// вкладка #/inbox вместе дают РОВНО ОДИН вызов `inbox_view`; повторный
// параллельный refresh присоединяется к inflight, а не считает снимок снова.
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App.vue";
import InboxView from "../views/InboxView.vue";
import { refreshWait, resetWaitStoreForTests, subscribeWait } from "../inbox";
import { snapshot } from "./fixtures";

const mocks = vi.hoisted(() => ({
  inboxView: vi.fn(),
  sessionInfo: vi.fn(),
}));

vi.mock("../api", () => ({
  api: { inboxView: mocks.inboxView, sessionInfo: mocks.sessionInfo },
  toUiError: (e: unknown) => e,
}));

const stubs = {
  RouterLink: { template: "<a><slot /></a>" },
  RouterView: { template: "<div />" },
};

beforeEach(() => {
  resetWaitStoreForTests();
  mocks.inboxView.mockReset();
  mocks.inboxView.mockResolvedValue(snapshot({ badge: 2 }));
  mocks.sessionInfo.mockResolvedValue({
    home: "/home/test",
    actor: "tester",
    opened_at: "2026-11-06T11:00:00Z",
  });
});

describe("shared wait snapshot (single polling source)", () => {
  it("header badge and the inbox tab together make exactly ONE inbox_view call", async () => {
    const shell = mount(App, { global: { stubs } });
    const view = mount(InboxView, { global: { stubs } });
    await flushPromises();
    expect(mocks.inboxView).toHaveBeenCalledTimes(1);
    expect(shell.text()).toContain("·");
    expect(shell.text()).toContain("2");
    shell.unmount();
    view.unmount();
  });

  it("concurrent refreshes join the inflight request", async () => {
    await Promise.all([refreshWait(), refreshWait(), refreshWait()]);
    expect(mocks.inboxView).toHaveBeenCalledTimes(1);
    await refreshWait();
    expect(mocks.inboxView).toHaveBeenCalledTimes(2);
  });

  it("two subscribers share one request and the timer stops with the last unsubscribe", async () => {
    const offA = subscribeWait();
    const offB = subscribeWait();
    await flushPromises();
    expect(mocks.inboxView).toHaveBeenCalledTimes(1);
    offA();
    offB();
    await refreshWait();
    expect(mocks.inboxView).toHaveBeenCalledTimes(2);
  });

  it("a failed refresh keeps the last published snapshot", async () => {
    mocks.inboxView.mockResolvedValueOnce(snapshot({ badge: 3 }));
    await refreshWait();
    mocks.inboxView.mockRejectedValueOnce({ code: "store", message: "нет стора" });
    await refreshWait();
    const { waitSnapshot, waitError } = await import("../inbox");
    expect(waitSnapshot.value?.badge).toBe(3);
    expect(waitError.value?.message).toBe("нет стора");
  });
});
