// Проведение приглашения на стороне приложения (task 01a1bf78).
//
// Проверяем именно то, что видит человек: страница спрашивает Rust «что делать»,
// сама задаёт машине ОДИН вопрос и докладывает статус — а решение (и адрес, и
// причина) приходит из Rust готовым. Причины и адреса здесь не изобретаются:
// иначе приложение и браузер разошлись бы в словах и в том, куда идти. Окно
// открывает адрес само (навигацией окна), поэтому второй загрузки — и второго
// расхода одноразового кода — не бывает.
import { describe, expect, it } from "vitest";
import {
  fetchPairStatus,
  runPairing,
  type PairBridge,
  type PairTask,
} from "../pairingNative";

const PROBE: PairTask = {
  kind: "probe",
  url: "http://192.168.1.20:4567/pair?token=abc",
  addr: "192.168.1.20:4567",
  timeout_ms: 1500,
};

function bridge(
  boot: PairTask,
  after: PairTask = { kind: "idle" },
  status: number | null = 200,
): PairBridge & { asked: string[]; reported: [string, number | null][] } {
  const asked: string[] = [];
  const reported: [string, number | null][] = [];
  return {
    asked,
    reported,
    boot: async () => boot,
    ask: async (url) => {
      asked.push(url);
      return status;
    },
    report: async (addr, read) => {
      reported.push([addr, read]);
      return after;
    },
  };
}

describe("приглашение внутри приложения", () => {
  it("вопрос задаётся ровно по адресу из Rust, а статус возвращается туда же", async () => {
    const home: PairTask = {
      kind: "go",
      url: "http://192.168.1.20:4567/?token=abc",
    };
    const fake = bridge(PROBE, home, 200);
    const done = await runPairing(fake);

    expect(fake.asked).toEqual([PROBE.url]);
    expect(fake.reported).toEqual([[PROBE.addr, 200]]);
    expect(done).toEqual(home);
  });

  it("молчание машины докладывается как «ответа нет», а не выдуманный статус", async () => {
    const refusal: PairTask = {
      kind: "go",
      url: "http://tauri.localhost/#/pair?reason=access_closed",
    };
    const fake = bridge(PROBE, refusal, null);
    const done = await runPairing(fake);

    expect(fake.asked).toEqual([PROBE.url]);
    expect(fake.reported).toEqual([[PROBE.addr, null]]);
    expect(done).toEqual(refusal);
  });

  it("«нечего делать» ничего не спрашивает и никуда не ходит", async () => {
    const fake = bridge({ kind: "idle" });
    const done = await runPairing(fake);
    expect(done.kind).toBe("idle");
    expect(fake.asked).toEqual([]);
    expect(fake.reported).toEqual([]);
  });

  it("задача без адреса вопроса не превращается в запрос «куда-нибудь»", async () => {
    const fake = bridge({ kind: "probe" });
    const done = await runPairing(fake);
    expect(done.kind).toBe("probe");
    expect(fake.asked).toEqual([]);
    expect(fake.reported).toEqual([]);
  });

  it("заметка «дом не запомнился» доезжает в адресе открываемой страницы", async () => {
    const withNotice: PairTask = {
      kind: "go",
      url: "http://192.168.1.20:4567/?token=abc&notice=pair_not_remembered",
    };
    const done = await runPairing(bridge(PROBE, withNotice, 200));
    expect(done).toEqual(withNotice);
    // Текст заметки объявлен в pairState и читается из адреса уже на новой
    // странице — модулю проводки не нужно ничего запоминать между загрузками.
  });
});

describe("один вопрос машине", () => {
  function answer(status: number): typeof fetch {
    return (async () => ({ ok: status === 200, status })) as unknown as typeof fetch;
  }

  it("статус читается как есть — решение принимает не страница", async () => {
    expect(await fetchPairStatus("http://m/pair", 1000, answer(200))).toBe(200);
    expect(await fetchPairStatus("http://m/pair", 1000, answer(401))).toBe(401);
    expect(await fetchPairStatus("http://m/pair", 1000, answer(403))).toBe(403);
  });

  it("оборванная связь — это «ответа нет», а не ошибка на экране", async () => {
    const dead = (async () => {
      throw new Error("connection refused");
    }) as unknown as typeof fetch;
    expect(await fetchPairStatus("http://m/pair", 1000, dead)).toBe(null);
  });
});
