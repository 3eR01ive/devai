// ОДНА сборка строки расхода (task 01a1b851, mt/dry): числа, слова «из …» и
// «без лимита» живут в `src/usage-format.ts`, а карточка, список и общий экран
// только зовут её. Тест пинует крайние случаи, на которых три прежние копии
// расходились: ноль, неизвестный расход и отсутствие лимита.
import assert from "node:assert/strict";
import { test } from "node:test";

import { limitText, money, spentText, taskSpendLine, tok } from "../src/usage-format.ts";

const usage = (over: Record<string, unknown> = {}) =>
  ({
    spent: 1_500_000,
    limit: 50_000_000 as number | null,
    unknown: 0,
    ...over,
  }) as never;

test("tok: ноль — «0», неизвестное — «—», крупные числа сокращаются", () => {
  assert.equal(tok(0), "0");
  assert.equal(tok(null), "—");
  assert.equal(tok(undefined), "—");
  assert.equal(tok(999), "999");
  assert.equal(tok(1_500), "2 тыс");
  assert.equal(tok(1_500_000), "1.5 млн");
  assert.equal(tok(2_000_000_000), "2.00 млрд");
});

test("spentText: неизвестный расход не выдаётся за ноль", () => {
  assert.equal(spentText(0, 1), "—", "ноль известных + неизвестные — не «0»");
  assert.equal(spentText(0, 0), "0", "известных прогонов нет — честный ноль");
  assert.equal(spentText(1_500_000, 2), "1.5 млн", "сумма известных — числом");
});

test("limitText и taskSpendLine: «без лимита» словами, не «из 0»", () => {
  assert.equal(limitText(usage({ limit: 50_000_000 })), "из 50.0 млн");
  assert.equal(limitText(usage({ limit: null })), "без лимита");
  assert.equal(taskSpendLine(usage()), "1.5 млн из 50.0 млн");
  assert.equal(
    taskSpendLine(usage({ limit: null })),
    "1.5 млн без лимита",
    "«0 из 0» недопустим",
  );
});

test("money: цена известна, цена неизвестна, прогонов нет", () => {
  assert.equal(money(0.25, 2), "$0.25");
  assert.equal(money(0, 2), "неизвестна");
  assert.equal(money(0, 0), "—");
});
