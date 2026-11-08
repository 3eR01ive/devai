// Раздел «Связка модели» (REQ-MS-06/07) на живом SFC: рендер снимка,
// ОДНО слово статуса, баннер отказа с предложением, проба и удержание
// кнопки применения. Слова проверяются по закрытому словарю — экран не
// имеет права придумать причину, которой нет в доказательстве.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import {
  buttonByLabel,
  elements,
  flush,
  installInvoke,
  loadView,
  mount,
  startHarness,
  textOf,
  type Harness,
} from "./support/component-harness.ts";
import {
  freshnessWord,
  holdCompletes,
  modelWord,
  providerWord,
  reasonWord,
  reasoningWord,
  statusWord,
} from "../src/model_view.ts";

let harness: Harness;

/** Poll until a condition holds — a loaded machine must not turn a real
 * completion into a flaky failure of a fixed sleep. */
async function waitUntil(cond: () => boolean, timeoutMs = 15_000): Promise<boolean> {
  const started = Date.now();
  while (!cond()) {
    if (Date.now() - started > timeoutMs) return false;
    await new Promise((r) => setTimeout(r, 20));
    await flush(1);
  }
  return true;
}

before(async () => {
  harness = await startHarness();
});
after(async () => {
  await harness?.close();
});

const snapshot = (overrides: Record<string, unknown> = {}) => ({
  snapshot_at: "2026-11-08T20:00:00Z",
  active: {
    id: "hermes-deepseek",
    label: "Hermes · DeepSeek",
    agent: "hermes",
    driver: "hermes",
    llm_provider: "deepseek",
    model: "deepseek-v4.1-flash",
    reasoning: "high",
    declared: true,
  },
  status: {
    state: "live",
    latency_ms: 820,
    verdict: "ok",
    reason: null,
    evidence: "probe",
    evidence_at: "2026-11-08T19:59:00Z",
    freshness: "fresh",
    probe_older: false,
  },
  declared: [
    {
      id: "hermes-deepseek",
      label: "Hermes · DeepSeek",
      agent: "hermes",
      driver: "hermes",
      llm_provider: "deepseek",
      model: "deepseek-v4.1-flash",
      reasoning: "high",
      probe_phrase: "ping",
      probe_timeout_ms: 20000,
      fresh_secs: 900,
      active: true,
    },
    {
      id: "hermes-alibaba",
      label: "Hermes · Alibaba",
      agent: "hermes",
      driver: "hermes",
      llm_provider: "alibaba",
      model: "qwen-max",
      reasoning: "medium",
      probe_phrase: "ping",
      probe_timeout_ms: 20000,
      fresh_secs: 900,
      active: false,
    },
  ],
  history: [],
  failures: null,
  running: [],
  ...overrides,
});

test("словарь статуса и причин — закрытый, без выдуманных причин", () => {
  assert.equal(statusWord("live"), "живая");
  assert.equal(statusWord("not_working"), "не работает");
  assert.equal(statusWord("not_proven"), "не доказано");
  assert.equal(statusWord("not_selected"), "связка не выбрана");
  assert.equal(statusWord("что-то"), "неизвестно");
  assert.equal(reasonWord("no_credits"), "нет кредитов");
  assert.equal(reasonWord("model_not_found"), "модель не существует");
  assert.equal(reasonWord("provider"), "отказ провайдера");
  assert.equal(reasonWord("самопридуманное"), "причина не распознана");
  assert.equal(reasonWord(null), "причина не распознана");
  // Пустые поля — словами, подстановки нет.
  assert.equal(providerWord(""), "не задан — выбирает драйвер");
  assert.equal(providerWord("local"), "не задан — выбирает драйвер");
  assert.equal(modelWord(""), "не передаётся");
  // Пустой уровень — неполная связка, а не «драйвер решит» (REQ-MS-09).
  assert.equal(reasoningWord(""), "не задан — связка неполна");
  assert.equal(reasoningWord("high"), "high");
  assert.equal(freshnessWord("unknown"), "доказательств нет");
  assert.equal(holdCompletes(699), false);
  assert.equal(holdCompletes(700), true);
});

test("ModelsView: снимок рендерит связку, ОДИН статус и задержку", async () => {
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return snapshot();
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();

  const text = textOf(root);
  assert.match(text, /Hermes · DeepSeek/);
  assert.match(text, /живая/);
  assert.match(text, /820 мс/);
  // Три поля раздельно и с подписями.
  assert.match(text, /драйвер/);
  assert.match(text, /LLM-провайдер/);
  assert.match(text, /модель/);
  // Уровень размышления — часть связки (REQ-MS-09), показан и подписан.
  assert.match(text, /уровень размышления/);
  assert.match(text, /high/);
  // Обе объявленные связки — списком целиком, без «+N ещё».
  assert.match(text, /hermes-alibaba/);
  assert.doesNotMatch(text, /\+N ещё/);
});

test("ModelsView: «не работает» горит баннером и предлагает живую связку", async () => {
  installInvoke((cmd) => {
    if (cmd === "model_snapshot")
      return snapshot({
        status: {
          state: "not_working",
          latency_ms: null,
          verdict: "no_credits",
          reason: "no_credits",
          evidence: "probe",
          evidence_at: "2026-11-08T19:59:00Z",
          freshness: "fresh",
          probe_older: false,
        },
        failures: {
          code: "no_credits",
          text: "связка не работает: нет кредитов — переключитесь на живую",
          suggestion: "hermes-alibaba",
        },
      });
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();

  const text = textOf(root);
  assert.match(text, /не работает/);
  assert.match(text, /нет кредитов/);
  assert.match(text, /hermes-alibaba/);
});

test("ModelsView: одиночный клик не применяет, удержание — применяет", async () => {
  let switches = 0;
  installInvoke((cmd, args) => {
    if (cmd === "model_snapshot") return snapshot();
    if (cmd === "model_switch") {
      switches += 1;
      assert.equal((args as { bundle_id: string }).bundle_id, "hermes-alibaba");
      return {
        outcome: "applied",
        bundle_id: "hermes-alibaba",
        verdict: "ok",
        latency_ms: 500,
        code: "applied",
        agent_changed: false,
      };
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();

  const buttons = elements(root).filter((n) => n.type === "button");
  assert.ok(buttons.length > 0);
  // Кнопки применения не имеют обработчика click вовсе: одиночный и двойной
  // клик ничего не применяют по построению.
  const holdButtons = buttons.filter((b) => textOf(b).includes("Удержать: переключить"));
  assert.equal(holdButtons.length, 1);
  assert.equal(holdButtons[0].props.onClick, undefined);

  const start = holdButtons[0].props.onPointerdown as (e: unknown) => void;
  start({});
  assert.ok(await waitUntil(() => switches === 1), "a completed hold applies exactly one switch");
  await flush();
  assert.equal(switches, 1, "a completed hold applies exactly one switch");
  assert.match(textOf(root), /переключено на hermes-alibaba/);
});

test("ModelsView: короткое нажатие отпускает без применения", async () => {
  let switches = 0;
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return snapshot();
    if (cmd === "model_switch") {
      switches += 1;
      return {
        outcome: "applied",
        bundle_id: "hermes-alibaba",
        verdict: "ok",
        latency_ms: 1,
        code: "applied",
        agent_changed: false,
      };
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();
  const hold = elements(root)
    .filter((n) => n.type === "button")
    .find((b) => textOf(b).includes("Удержать: переключить"));
  assert.ok(hold);
  (hold.props.onPointerdown as (e: unknown) => void)({});
  (hold.props.onPointerup as (e: unknown) => void)({});
  await new Promise((r) => setTimeout(r, 1_200));
  await flush();
  assert.equal(switches, 0, "a released hold applies nothing");
});

test("ModelsView: смена имени агента называет идущие прогоны из снимка", async () => {
  const snap = snapshot({
    running: [
      {
        execution_id: "01a1bceb-aaaa",
        task_id: "01a1bceb-bbbb",
        agent: "hermes",
        started_at: "2026-11-08T19:00:00Z",
      },
    ],
  });
  let switches = 0;
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return snap;
    if (cmd === "model_switch") {
      switches += 1;
      return {
        outcome: "applied",
        bundle_id: "hermes-alibaba",
        verdict: "ok",
        latency_ms: 90,
        code: "applied",
        agent_changed: true,
      };
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();

  const hold = elements(root)
    .filter((n) => n.type === "button")
    .find((b) => textOf(b).includes("Удержать: переключить"));
  assert.ok(hold);
  (hold.props.onPointerdown as (e: unknown) => void)({});
  assert.ok(await waitUntil(() => switches === 1), "the hold must apply");
  await flush();
  const text = textOf(root);
  assert.equal(switches, 1, `the hold must apply: ${text}`);
  assert.match(text, /идущие прогоны остаются на прежнем агенте/);
  assert.match(text, /hermes/, "the names come from the snapshot's running rows");
});
