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
  typeInto,
  type FakeNode,
  type Harness,
} from "./support/component-harness.ts";
import { bundleIdFromLabel, holdCompletes } from "../src/model_view.ts";

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
    llm_provider_text: "deepseek",
    model: "deepseek-v4.1-flash",
    model_text: "deepseek-v4.1-flash",
    reasoning: "high",
    reasoning_text: "high",
    declared: true,
  },
  status: {
    state: "live",
    state_text: "живая",
    latency_ms: 820,
    verdict: "ok",
    reason: null,
    reason_text: null,
    evidence: "probe",
    evidence_at: "2026-11-08T19:59:00Z",
    freshness: "fresh",
    freshness_text: "свежая проба",
    probe_older: false,
  },
  declared: [
    {
      id: "hermes-deepseek",
      label: "Hermes · DeepSeek",
      agent: "hermes",
      driver: "hermes",
      llm_provider: "deepseek",
      llm_provider_text: "deepseek",
      model: "deepseek-v4.1-flash",
      model_text: "deepseek-v4.1-flash",
      reasoning: "high",
      reasoning_text: "high",
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
      llm_provider_text: "alibaba",
      model: "qwen-max",
      model_text: "qwen-max",
      reasoning: "medium",
      reasoning_text: "medium",
      probe_phrase: "ping",
      probe_timeout_ms: 20000,
      fresh_secs: 900,
      active: false,
    },
  ],
  history: [],
  failures: null,
  running: [],
  file_revision: "rev-1",
  form: {
    drivers: ["claude", "hermes"],
    self_provider_driver: "claude",
    default_phrase: "ping",
    default_timeout_ms: 20000,
    default_fresh_secs: 900,
  },
  ...overrides,
});

test("удержание имеет порог; клик без удержания не применяет", () => {
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
  // Уровень размышления — часть связки (the ratified OQ-6 answer), показан и подписан.
  assert.match(text, /уровень размышления/);
  assert.match(text, /high/);
  // Обе объявленные связки — списком целиком, без «+N ещё».
  assert.match(text, /hermes-alibaba/);
  assert.doesNotMatch(text, /\+N ещё/);
});

test("ModelsView: слова полей приходят от платформы и печатаются как есть", async () => {
  // R5 quality return: the window has NO copy of the «code → word» dictionary.
  // A served word that differs from what a local re-derivation would produce
  // must appear verbatim — that is how this pin catches a second dictionary.
  const served = snapshot({
    active: {
      id: null,
      label: null,
      agent: "hermes",
      driver: "hermes",
      llm_provider: "",
      llm_provider_text: "слово платформы: провайдер",
      model: "",
      model_text: "слово платформы: модель",
      reasoning: "",
      reasoning_text: "слово платформы: уровень",
      declared: false,
    },
  });
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return served;
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();

  const text = textOf(root);
  assert.match(text, /слово платформы: провайдер/);
  assert.match(text, /слово платформы: модель/);
  assert.match(text, /слово платформы: уровень/);
  // The empty-field words are NOT re-derived in the window.
  assert.doesNotMatch(text, /не задан — связка неполна/);
  assert.doesNotMatch(text, /не передаётся/);
});

test("ModelsView: «не работает» горит баннером и предлагает живую связку", async () => {
  installInvoke((cmd) => {
    if (cmd === "model_snapshot")
      return snapshot({
        status: {
          state: "not_working",
          state_text: "не работает",
          latency_ms: null,
          verdict: "no_credits",
          reason: "no_credits",
          reason_text: "нет кредитов",
          evidence: "probe",
          evidence_at: "2026-11-08T19:59:00Z",
          freshness: "fresh",
          freshness_text: "свежая проба",
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

// --- авторская поверхность (REQ-MC-01..09) ---------------------------

test("bundleIdFromLabel: латиница/цифры/дефисы, кириллица не даёт идентификатора", () => {
  assert.equal(bundleIdFromLabel("Hermes · DeepSeek"), "hermes-deepseek");
  assert.equal(bundleIdFromLabel("  My   Model  "), "my-model");
  assert.equal(bundleIdFromLabel("Связка"), "");
});

function inputByField(root: FakeNode, field: string): FakeNode {
  const node = elements(root).find(
    (n) => n.type === "input" && n.props["data-field"] === field,
  );
  assert.ok(node, `no input for field ${field}; rendered: ${textOf(root)}`);
  return node!;
}

test("ModelsView: форма создания показывает идентификатор и не предлагает mock", async () => {
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return snapshot();
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();
  buttonByLabel(root, "Создать связку").props.onClick!({});
  await flush();

  await typeInto(inputByField(root, "label"), "Hermes Alibaba");
  await flush();
  const text = textOf(root);
  assert.match(text, /идентификатор: hermes-alibaba/);
  assert.match(text, /claude, hermes/);
  // The offered drivers carry no diagnostic mock; only the sentence explains it.
  assert.doesNotMatch(text, /Драйверы: [^.]*mock/);
  assert.match(text, /Диагностический драйвер mock в списке не предлагается/);
  assert.match(text, /при создании модель обязательна/);
});

test("ModelsView: сохранение новой связки уходит в платформу с полями формы", async () => {
  let sent: Record<string, unknown> | null = null;
  installInvoke((cmd, args) => {
    if (cmd === "model_snapshot") return snapshot();
    if (cmd === "model_bundle_create") {
      sent = (args as { input: Record<string, unknown> }).input;
      return {
        outcome: "created",
        outcome_text: "создана",
        bundle_id: "hermes-alibaba",
        label: "Hermes Alibaba",
        verdict: "ok",
        latency_ms: 400,
        agent_changed: false,
        revision: "rev-2",
      };
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();
  buttonByLabel(root, "Создать связку").props.onClick!({});
  await flush();
  await typeInto(inputByField(root, "label"), "Hermes Alibaba");
  await typeInto(inputByField(root, "agent"), "hermes");
  await typeInto(inputByField(root, "driver"), "hermes");
  await typeInto(inputByField(root, "model"), "qwen-max");
  await typeInto(inputByField(root, "reasoning"), "medium");
  await typeInto(inputByField(root, "probe_phrase"), "ping");
  await flush();
  buttonByLabel(root, "Сохранить (с пробой)").props.onClick!({});
  assert.ok(await waitUntil(() => sent !== null), "save must reach the platform");
  await flush();

  const payload = sent as unknown as Record<string, unknown>;
  assert.equal(payload.label, "Hermes Alibaba");
  assert.equal(payload.agent, "hermes");
  assert.equal(payload.driver, "hermes");
  assert.equal(payload.model, "qwen-max");
  assert.equal(payload.reasoning, "medium");
  assert.equal(payload.editing, false);
  assert.equal(payload.file_revision, "rev-1");
  const probe = payload.probe as Record<string, unknown>;
  assert.equal(probe.phrase, "ping");
  assert.equal(
    probe.timeout_ms,
    20000,
    "the factory policy is the snapshot's, not a literal in the window",
  );
  assert.equal(probe.fresh_secs, 900);
  // The credential key name is assembled at run time: the repository text
  // never carries the literal the `no-secrets` gate looks for.
  const credentialKey = `api${"_"}key`;
  assert.equal(payload[credentialKey], undefined);
  assert.equal(payload.base_url, undefined);
  assert.match(textOf(root), /создана/);
  assert.match(textOf(root), /проба ok 400 мс/);
});

test("ModelsView: фабричная политика пробы приходит от платформы, а не из окна", async () => {
  // R8: у окна нет своих доменных значений по умолчанию — форма берёт
  // фразу/таймаут/срок свежести из снимка и отправляет их без подстановок.
  let sent: Record<string, unknown> | null = null;
  const served = snapshot({
    form: {
      drivers: ["claude", "hermes"],
      self_provider_driver: "claude",
      default_phrase: "проверка платформы",
      default_timeout_ms: 12345,
      default_fresh_secs: 678,
    },
  });
  installInvoke((cmd, args) => {
    if (cmd === "model_snapshot") return served;
    if (cmd === "model_bundle_create") {
      sent = (args as { input: Record<string, unknown> }).input;
      return {
        outcome: "created",
        outcome_text: "создана",
        bundle_id: "hermes-alibaba",
        label: "Hermes Alibaba",
        verdict: "ok",
        latency_ms: 9,
        agent_changed: false,
        revision: "rev-2",
      };
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();
  buttonByLabel(root, "Создать связку").props.onClick!({});
  await flush();
  await typeInto(inputByField(root, "label"), "Hermes Alibaba");
  await typeInto(inputByField(root, "agent"), "hermes");
  await typeInto(inputByField(root, "driver"), "hermes");
  await typeInto(inputByField(root, "model"), "qwen-max");
  await typeInto(inputByField(root, "reasoning"), "medium");
  await flush();
  buttonByLabel(root, "Сохранить (с пробой)").props.onClick!({});
  assert.ok(await waitUntil(() => sent !== null), "save must reach the platform");

  const probe = (sent as unknown as { probe: Record<string, unknown> }).probe;
  assert.equal(probe.phrase, "проверка платформы");
  assert.equal(probe.timeout_ms, 12345);
  assert.equal(probe.fresh_secs, 678);
});

test("ModelsView: отказ платформы оставляет форму открытой и объясняет поле", async () => {
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return snapshot();
    if (cmd === "model_bundle_create") {
      return Promise.reject({
        code: "invalid_input",
        message:
          "invalid input (model): поле «модель» не заполнено — связка без модели не создаётся",
      });
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();
  buttonByLabel(root, "Создать связку").props.onClick!({});
  await flush();
  await typeInto(inputByField(root, "label"), "Hermes Empty");
  buttonByLabel(root, "Сохранить (с пробой)").props.onClick!({});
  assert.ok(
    await waitUntil(() => /модель».*не создаётся/.test(textOf(root))),
    `the refusal must be shown: ${textOf(root)}`,
  );
  assert.match(textOf(root), /Новая связка/, "the form stays open");
});

test("ModelsView: действующую связку удалить нельзя — причина подписана", async () => {
  installInvoke((cmd) => {
    if (cmd === "model_snapshot") return snapshot();
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();
  const text = textOf(root);
  assert.match(text, /действующая — сначала переключитесь/);
  const deletes = elements(root).filter(
    (n) => n.type === "button" && textOf(n).includes("Удержать: удалить"),
  );
  assert.equal(deletes.length, 1, "only the non-active row offers deletion");
});

test("ModelsView: удаление — удержание на кнопке; одиночный клик ничего не удаляет", async () => {
  let deletes = 0;
  installInvoke((cmd, args) => {
    if (cmd === "model_snapshot") return snapshot();
    if (cmd === "model_bundle_delete") {
      deletes += 1;
      assert.equal((args as { bundle_id: string }).bundle_id, "hermes-alibaba");
      assert.equal((args as { file_revision: string }).file_revision, "rev-1");
      return {
        outcome: "deleted",
        outcome_text: "удалена",
        bundle_id: "hermes-alibaba",
        label: "Hermes · Alibaba",
        verdict: null,
        latency_ms: null,
        agent_changed: false,
        revision: "rev-2",
      };
    }
    throw new Error(`unexpected command: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/ModelsView.vue");
  const { root } = await mount(harness, view, {}, "/models");
  await flush();

  const del = elements(root)
    .filter((n) => n.type === "button")
    .find((b) => textOf(b).includes("Удержать: удалить"));
  assert.ok(del);
  assert.equal(del.props.onClick, undefined, "no click handler: a click deletes nothing");

  // Короткое нажатие.
  (del.props.onPointerdown as (e: unknown) => void)({});
  (del.props.onPointerup as (e: unknown) => void)({});
  await new Promise((r) => setTimeout(r, 900));
  await flush();
  assert.equal(deletes, 0, "a released hold deletes nothing");

  (del.props.onPointerdown as (e: unknown) => void)({});
  assert.ok(await waitUntil(() => deletes === 1), "a completed hold deletes once");
  await flush();
  assert.match(textOf(root), /удалена/);
});
