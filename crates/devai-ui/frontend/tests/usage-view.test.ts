// Экран расхода токенов (task 01a1b851, REQ-USG-08) на живом SFC: сутки и
// месяц, «неизвестно» ЧИСЛОМ прогонов (не нулём), топ-задачи, честное
// «данных мало» вместо выдуманного прогноза и «без лимита» словами.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import {
  flush,
  installInvoke,
  loadView,
  mount,
  startHarness,
  textOf,
  type Harness,
} from "./support/component-harness.ts";

let harness: Harness;

before(async () => {
  harness = await startHarness();
});
after(async () => {
  await harness?.close();
});

const bucket = (over: Record<string, unknown> = {}) => ({
  key: "",
  name: null as string | null,
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  runs: 0,
  measured: 0,
  unknown: 0,
  cost_unknown: 0,
  ...over,
});

const limits = (over: Record<string, unknown> = {}) => ({
  run: 8_000_000,
  task: 50_000_000,
  day: 200_000_000,
  raised_to: null,
  ...over,
});

const summary = (over: Record<string, unknown> = {}) => ({
  period: "day",
  from: "2026-11-15T00:00:00Z",
  to: "2026-11-15T12:00:00Z",
  group_by: "totals",
  rows: [],
  totals: bucket(),
  limits: limits(),
  ...over,
});

function overview(over: Record<string, unknown> = {}) {
  return {
    day: summary({
      totals: bucket({
        prompt_tokens: 1_400_000,
        completion_tokens: 100_000,
        total_tokens: 1_500_000,
        cost_usd: 0.25,
        runs: 3,
        measured: 2,
        unknown: 1,
      }),
    }),
    month: summary({
      period: "month",
      totals: bucket({ total_tokens: 2_000_000, runs: 5, measured: 4, unknown: 1 }),
    }),
    top_tasks: [
      bucket({
        key: "01a1b851-6864-7368-a260-231cb61931c0",
        name: "Контроль токенов",
        total_tokens: 1_500_000,
        cost_usd: 0.25,
        runs: 3,
        measured: 2,
        unknown: 1,
      }),
    ],
    forecast: {
      available: false,
      // The honest «not enough data», naming what is missing.
      line: "данных мало: за сутки 2 прогонов с известным расходом, для прогноза нужно не меньше 3",
      runs_used: 2,
      needed_runs: 3,
    },
    day_gate: null as string | null,
    limits: limits(),
    ...over,
  };
}

async function render(view: unknown, path: string) {
  const loaded = await loadView(harness, view as never);
  const { root } = await mount(harness, loaded, {}, path);
  await flush();
  return textOf(root).replace(/\s+/g, " ");
}

test("UsageView: сутки, месяц и топ-задачи читаются с одного снимка", async () => {
  installInvoke((cmd) => (cmd === "usage_overview" ? overview() : null));
  const text = await render("/src/views/UsageView.vue", "/usage");

  assert.match(text, /Расход токенов/);
  assert.match(text, /1\.5 млн/, `сутки: ${text}`);
  assert.match(text, /2\.0 млн/, `месяц: ${text}`);
  // «Неизвестно» — ЧИСЛОМ прогонов рядом с суммой, никогда нулём.
  assert.match(text, /расход неизвестен у 1/, text);
  // Топ-задачи: имя и сумма.
  assert.match(text, /Контроль токенов/, text);
  assert.match(text, /прогонов 3/, text);
  // Прогноз без данных — честная оговорка, а не выдуманный горизонт.
  assert.match(text, /Прогноз недоступен/, text);
  assert.match(text, /данных мало/, text);
  assert.match(text, /порог 3/, text);
});

test("UsageView: «без лимита» и суточный гейт названы словами", async () => {
  const noLimits = overview({
    limits: limits({ run: null, task: null, day: null }),
    day_gate:
      "суточный бюджет исчерпан: за сутки израсходовано 200000000 из 200000000 — новые прогоны не запускаются до новых суток (уже начатые досчитают)",
  });
  installInvoke((cmd) => (cmd === "usage_overview" ? noLimits : null));
  const text = await render("/src/views/UsageView.vue", "/usage");

  assert.match(text, /без лимита/, `«0» вместо слов недопустим: ${text}`);
  assert.match(text, /суточный бюджет исчерпан/, text);
  assert.match(text, /до новых суток/, text);
});
