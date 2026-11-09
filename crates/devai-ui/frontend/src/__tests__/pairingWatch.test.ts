// Тёплый скан (task 01a1bf78, quality return R1): приложение уже открыто и
// показывает СВОЮ страницу — сканирование меняет только фрагмент адреса, и
// корень окна второй раз не монтируется. Значит обмен должен запускать ещё и
// слушатель смены адреса; этот тест проходит ровно путь человека:
// «приложение открыто → скан → пара достигнута», без перезагрузки документа.
import { flushPromises } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import {
  isConnectHash,
  pairingRunner,
  pairThisScreen,
  requestPairingRun,
  resetPairingWatchForTests,
  setPairingRunner,
  watchScans,
  type PairingWindow,
} from "../pairingWatch";
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

function bridge(after: PairTask): PairBridge & { asked: string[]; boots: number } {
  const fake = {
    asked: [] as string[],
    boots: 0,
    boot: async () => {
      fake.boots += 1;
      return PROBE;
    },
    ask: async (url: string) => {
      fake.asked.push(url);
      return 200;
    },
    report: async () => after,
  };
  return fake;
}

/// Окно с адресом и настоящей очередью слушателей: `go` меняет адрес и
/// рассылает `hashchange`, как это делает браузер.
function fakeWindow(hash: string) {
  const listeners = new Set<() => void>();
  const location = { hash };
  const host: PairingWindow & {
    go: (next: string) => void;
    listeners: () => number;
  } = {
    location,
    addEventListener: (_type: "hashchange", listener: () => void) => {
      listeners.add(listener);
    },
    removeEventListener: (_type: "hashchange", listener: () => void) => {
      listeners.delete(listener);
    },
    go: (next: string) => {
      location.hash = next;
      for (const listener of listeners) listener();
    },
    listeners: () => listeners.size,
  };
  return host;
}

describe("тёплый скан внутри уже открытого приложения", () => {
  it("смена адреса на экран скана доводит до дома этой машины", async () => {
    const host = fakeWindow("#/projects");
    const fake = bridge(HOME);
    const reached: PairTask[] = [];
    const run = pairingRunner(
      fake,
      (task) => reached.push(task),
      (error) => {
        throw error;
      },
    );
    const stop = watchScans(host, run);
    expect(fake.boots).toBe(0);

    host.go("#/pair?step=connect&scan=0123456789abcdef");
    await flushPromises();

    expect(fake.boots).toBe(1);
    expect(fake.asked).toEqual([PROBE.url]);
    expect(reached).toEqual([HOME]);
    expect(reached[0].url).toBe("http://192.168.1.20:4567/?token=abc");
    stop();
    expect(host.listeners()).toBe(0);
  });

  it("обычный переход по разделам окна обмен не запускает", async () => {
    const host = fakeWindow("#/projects");
    const fake = bridge(HOME);
    const run = pairingRunner(fake, () => {}, () => {});
    watchScans(host, run);
    host.go("#/tasks");
    await flushPromises();
    expect(fake.boots).toBe(0);
    expect(fake.asked).toEqual([]);
  });

  it("экран не про скан не спрашивает машину вовсе", async () => {
    const fake = bridge(HOME);
    expect(await pairThisScreen("#/pair?reason=wrong_network", fake)).toBe(null);
    expect(fake.boots).toBe(0);
    expect(isConnectHash("#/pair?step=connect&scan=abc")).toBe(true);
    expect(isConnectHash("#/pair?step=connect")).toBe(true);
    expect(isConnectHash("#/projects")).toBe(false);
  });

  it("два быстрых скана не ведут два обмена параллельно", async () => {
    const fake = bridge(HOME);
    const run = pairingRunner(fake, () => {}, () => {});
    run();
    run();
    await flushPromises();
    expect(fake.boots).toBe(1);
    expect(fake.asked).toHaveLength(1);
  });
});

describe("кнопка «продолжить» на экране скана", () => {
  it("просит ровно тот проход, который зарегистрировал корень окна", () => {
    let runs = 0;
    setPairingRunner(() => {
      runs += 1;
    });
    requestPairingRun();
    expect(runs).toBe(1);
    resetPairingWatchForTests();
    requestPairingRun();
    expect(runs).toBe(1);
  });
});
