// Сторож связи с домом на телефоне (task 01a1bf78, REQ-QRAPP-05/06):
// страница сама замечает отзыв доступа дешёвым вопросом GET /pair, показывает
// причину словами и перестаёт спрашивать по протухшему коду.
import { afterEach, describe, expect, it } from "vitest";
import {
  checkPairing,
  PAIR_POLL_MS,
  pairFailure,
  pairReasonForAnswer,
  probePairing,
  readNotice,
  resetPairingForTests,
} from "../pairing";
import { PAIR_REASONS, pairNotice, pairRefusal } from "../pairState";

function answer(status: number): typeof fetch {
  return (async () => ({ ok: status === 200, status })) as unknown as typeof fetch;
}

function dead(): typeof fetch {
  return (async () => {
    throw new Error("connection refused");
  }) as unknown as typeof fetch;
}

afterEach(() => resetPairingForTests());

describe("сторож связи", () => {
  it("живой сессии ничего не выдумывает", async () => {
    expect(PAIR_POLL_MS).toBeLessThanOrEqual(5000);
    expect(await probePairing(answer(200))).toBe("paired");
    expect(pairReasonForAnswer("paired")).toBe(null);
    await checkPairing(answer(200));
    expect(pairFailure.value).toBe(null);
  });

  it("ответ машины «не пущу» — это «доступ закрыт», а не общая ошибка связи", async () => {
    expect(await probePairing(answer(401))).toBe("refused");
    expect(await probePairing(answer(403))).toBe("refused");
    await checkPairing(answer(401));
    expect(pairFailure.value?.code).toBe("access_closed");
    expect(pairFailure.value?.title).toBe("доступ закрыт");
  });

  it("молчание адреса в сети машины читается как закрытая дверь", async () => {
    expect(await probePairing(dead())).toBe("silent");
    expect(pairReasonForAnswer("silent")).toBe("access_closed");
    await checkPairing(dead());
    expect(pairFailure.value?.title).toBe("доступ закрыт");
  });

  it("после отказа по старому коду запросов больше нет", async () => {
    await checkPairing(answer(401));
    let calls = 0;
    const counting = (async () => {
      calls += 1;
      return { ok: true, status: 200 };
    }) as unknown as typeof fetch;
    await checkPairing(counting);
    expect(calls).toBe(0);
    expect(pairFailure.value?.code).toBe("access_closed");
  });

  it("каждая причина в словаре имеет свою фразу, и они различаются", async () => {
    const titles = PAIR_REASONS.map((reason) => pairRefusal(reason)?.title);
    expect(new Set(titles).size).toBe(PAIR_REASONS.length);
    expect(titles).toContain("приглашение просрочено");
    expect(titles).toContain("не та сеть");
    expect(titles).toContain("доступ закрыт");
    expect(pairRefusal("unknown")).toBe(null);
  });

  it("заметка о незапомненном доме читается из адреса и имеет свои слова", async () => {
    // Код объявляет Rust (`PairStoreError::CODE`), слова — здесь.
    expect(readNotice("?token=abc&notice=pair_not_remembered")).toBe("pair_not_remembered");
    expect(readNotice("?token=abc")).toBe(null);
    expect(readNotice("?notice=")).toBe(null);
    const notice = pairNotice("pair_not_remembered");
    expect(notice?.title).toBe("дом не запомнится");
    expect(notice?.hint.length).toBeGreaterThan(0);
    // Неизвестный код не превращается в выдуманное предупреждение.
    expect(pairNotice("something_else")).toBe(null);
  });
});
