// Тёплый скан на уровне настоящего корня окна (task 01a1bf78, quality return
// R1): приложение УЖЕ открыто и показывает свой раздел, приходит сканирование —
// окно меняет только фрагмент адреса, корень второй раз не монтируется.
// Тест монтирует App, как это делает приложение, и проверяет, что после смены
// адреса обмен доходит до пары — то есть человек попадает в дом, а не остаётся
// на «подключаюсь к дому…».
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App.vue";
import { resetPairingWatchForTests } from "../pairingWatch";
import type { PairBridge, PairTask } from "../pairingNative";

const PROBE: PairTask = {
  kind: "probe",
  url: "http://192.168.1.20:4567/pair?token=abc",
  addr: "192.168.1.20:4567",
  timeout_ms: 1500,
};
const HOME: PairTask = {
  kind: "go",
  url: "http://192.168.1.20:4567/?token=abc",
};

const mocks = vi.hoisted(() => ({
  boot: vi.fn(),
  ask: vi.fn(),
  report: vi.fn(),
  sessionInfo: vi.fn(),
  inboxView: vi.fn(),
  taskActive: vi.fn(async () => ({ snapshot_at: "", rows: [] })),
}));

vi.mock("../api", () => ({
  api: { sessionInfo: mocks.sessionInfo, inboxView: mocks.inboxView, taskActive: mocks.taskActive },
  toUiError: (e: unknown) => e,
  // Это СВОЯ страница приложения: мост в Rust есть, сторож дома не нужен.
  pageWatchesPairing: false,
  pairingShell: true,
}));

vi.mock("../pairingNative", () => ({
  windowBridge: (): PairBridge => ({
    boot: mocks.boot,
    ask: mocks.ask,
    report: mocks.report,
  }),
  // Тот же проход, что в настоящем модуле; сеть и Tauri заменены двойниками.
  runPairing: async (bridge: PairBridge): Promise<PairTask> => {
    const first = await bridge.boot();
    if (first.kind !== "probe" || !first.url || !first.addr) return first;
    const status = await bridge.ask(first.url, first.timeout_ms ?? 0);
    return bridge.report(first.addr, status);
  },
}));

const stubs = {
  RouterLink: { template: "<a><slot /></a>" },
  RouterView: { template: "<div />" },
};

beforeEach(() => {
  resetPairingWatchForTests();
  mocks.boot.mockReset();
  mocks.ask.mockReset();
  mocks.report.mockReset();
  mocks.sessionInfo.mockResolvedValue({
    home: "/home/test",
    actor: "tester",
    opened_at: "2026-11-09T11:00:00Z",
  });
  mocks.inboxView.mockResolvedValue({ badge: 0, tasks: [], stopped: [], notes: [] });
});

describe("приложение уже открыто, приходит скан", () => {
  it("смена адреса запускает обмен и доводит до дома", async () => {
    // Первый проход при монтировании: ссылки нет — делать нечего.
    mocks.boot.mockResolvedValueOnce({ kind: "idle" } as PairTask);
    mocks.ask.mockResolvedValue(200);
    mocks.report.mockResolvedValue(HOME);

    const app = mount(App, { global: { stubs } });
    await flushPromises();
    expect(mocks.boot).toHaveBeenCalledTimes(1);
    expect(mocks.report).not.toHaveBeenCalled();

    // Скан: Rust поднимает экран скана сменой ТОЛЬКО фрагмента адреса.
    // `pushState` меняет адрес без собственного события, чтобы тест сам
    // решил, когда браузер пришлёт `hashchange` (иначе jsdom пришлёт второе).
    mocks.boot.mockResolvedValueOnce(PROBE);
    window.history.pushState(null, "", "#/pair?step=connect&scan=0123456789abcdef");
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await flushPromises();

    expect(mocks.boot).toHaveBeenCalledTimes(2);
    expect(mocks.ask).toHaveBeenCalledWith(PROBE.url, PROBE.timeout_ms);
    expect(mocks.report).toHaveBeenCalledWith(PROBE.addr, 200);
    app.unmount();
    window.history.replaceState(null, "", "#/");
  });
});
