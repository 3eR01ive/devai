// Панель «Доступ с телефона» (task 01a1be98): состояние — факт сервера
// (off / awaiting / connected), а не намерение; QR и одноразовый код видны
// только пока приглашение не использовано; подключённое устройство видно
// с именем и адресом. Сеть/IPC замоканы — проверяется отрисовка фактов.
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import LanView from "../views/LanView.vue";
import type { LanStatus } from "../types";

const mocks = vi.hoisted(() => ({
  lanStatus: vi.fn(),
  lanEnable: vi.fn(),
  lanDisable: vi.fn(),
  lanNewInvite: vi.fn(),
  lanDisconnect: vi.fn(),
}));

vi.mock("../api", () => ({
  api: {
    lanStatus: mocks.lanStatus,
    lanEnable: mocks.lanEnable,
    lanDisable: mocks.lanDisable,
    lanNewInvite: mocks.lanNewInvite,
    lanDisconnect: mocks.lanDisconnect,
  },
  toUiError: (e: unknown) => e,
}));

function status(over: Partial<LanStatus>): LanStatus {
  return {
    state: "off",
    address: null,
    url: null,
    token: null,
    qr_svg: null,
    reason_code: null,
    reason: null,
    devices: [],
    started_at: null,
    ...over,
  };
}

beforeEach(() => {
  for (const fn of Object.values(mocks)) fn.mockReset();
});

describe("доступ с телефона", () => {
  it("выключенный доступ называет причину и не показывает QR", async () => {
    mocks.lanStatus.mockResolvedValue(
      status({ reason_code: "network_changed", reason: "сеть изменилась" }),
    );
    const view = mount(LanView);
    await flushPromises();
    expect(view.get("[data-testid='lan-state']").text()).toContain("выключен");
    expect(view.get("[data-testid='lan-reason']").text()).toContain("network_changed");
    expect(view.find("[data-testid='lan-qr']").exists()).toBe(false);
    view.unmount();
  });

  it("ждёт подключения: адрес, одноразовый код и QR — на месте", async () => {
    mocks.lanStatus.mockResolvedValue(
      status({
        state: "awaiting",
        address: "192.168.1.20:41234",
        url: "http://192.168.1.20:41234/?token=abc",
        token: "abc",
        qr_svg: "<svg data-testid='qr'></svg>",
      }),
    );
    const view = mount(LanView);
    await flushPromises();
    expect(view.get("[data-testid='lan-state']").text()).toContain("ждёт подключения");
    expect(view.get("[data-testid='lan-address']").text()).toContain("192.168.1.20:41234");
    expect(view.get("[data-testid='lan-token']").text()).toBe("abc");
    expect(view.find("[data-testid='lan-qr'] svg").exists()).toBe(true);
    view.unmount();
  });

  it("подключённый телефон виден по имени и адресу, без QR", async () => {
    mocks.lanStatus.mockResolvedValue(
      status({
        state: "connected",
        address: "192.168.1.20:41234",
        devices: [
          {
            id: "s1",
            name: "Linux; Android 14; Pixel 7",
            addr: "192.168.1.42",
            connected_at: "2026-11-09T10:00:00Z",
            last_seen: "2026-11-09T10:01:00Z",
            live: true,
          },
        ],
      }),
    );
    const view = mount(LanView);
    await flushPromises();
    expect(view.get("[data-testid='lan-state']").text()).toContain("подключено");
    expect(view.text()).toContain("Pixel 7");
    expect(view.text()).toContain("192.168.1.42");
    expect(view.find("[data-testid='lan-qr']").exists()).toBe(false);
    view.unmount();
  });

  it("кнопка «включить» — один вызов и смена состояния на факт ответа", async () => {
    mocks.lanStatus.mockResolvedValue(status({}));
    mocks.lanEnable.mockResolvedValue(
      status({ state: "awaiting", address: "10.0.0.5:5555", token: "t", url: "u", qr_svg: "<svg/>" }),
    );
    const view = mount(LanView);
    await flushPromises();
    const button = view.findAll("button").find((b) => b.text().includes("Включить доступ"));
    expect(button).toBeTruthy();
    await button!.trigger("click");
    await flushPromises();
    expect(mocks.lanEnable).toHaveBeenCalledTimes(1);
    expect(view.get("[data-testid='lan-state']").text()).toContain("ждёт подключения");
    view.unmount();
  });
});
