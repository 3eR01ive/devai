// R3 quality return: HomesView add()/pick()/toggle() and the merged-view
// home filter were only executed by hand. Here the SFCs are mounted through
// the component harness (real template, real handlers) over a fake IPC, and
// the asserted facts are the operator's: which API calls were issued with
// which arguments and how the rendered rows followed them.

import assert from "node:assert/strict";
import { after, before, beforeEach, test } from "node:test";

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

const ACTIVE_HOME = "/srv/devai/.devai";
const TRIPSALES_HOME = "/srv/tripsales/services/.devai";
const PICKED_HOME = "/srv/picked/.devai";

function home(path: string, over: Record<string, unknown> = {}) {
  return {
    path,
    project: path.includes("tripsales") ? "tripsales" : "devai",
    added_at: "2026-11-06T12:00:00Z",
    enabled: true,
    active: false,
    available: true,
    schema_version: 8,
    version_note: null,
    read_error: null,
    ...over,
  };
}

function block(home_path: string, records: unknown[]) {
  return {
    snapshot_at: "2026-11-06T12:00:00Z",
    home_path,
    home_project: home_path.includes("tripsales") ? "tripsales" : "devai",
    is_active: false,
    available: true,
    version_note: null,
    error: null,
    records,
  };
}

function project(id: string, name: string) {
  return {
    id,
    name,
    description: "",
    repos: [],
    created_at: "2026-11-06T12:00:00Z",
    updated_at: "2026-11-06T12:00:00Z",
  };
}

type Args = Record<string, unknown>;

let harness: Harness;
let rows: ReturnType<typeof home>[];
let addCalls: string[];
let toggleCalls: { path: string; enabled: boolean }[];
let addFailure: { code: string; message: string } | null;
let listFailure: { code: string; message: string } | null;
let listCalls: number;
let sessionHome: string;

function wireHomes() {
  rows = [home(ACTIVE_HOME, { active: true }), home(TRIPSALES_HOME)];
  addCalls = [];
  toggleCalls = [];
  addFailure = null;
  listFailure = null;
  listCalls = 0;
  sessionHome = ACTIVE_HOME;
  installInvoke((cmd, raw) => {
    const args = (raw ?? {}) as Args;
    switch (cmd) {
      case "homes_list":
        listCalls += 1;
        if (listFailure) throw listFailure;
        return [...rows];
      case "homes_add": {
        const path = args.path as string;
        addCalls.push(path);
        if (addFailure) throw addFailure;
        const record = home(path);
        rows.push(record);
        return record;
      }
      case "homes_set_enabled": {
        const path = args.path as string;
        const enabled = args.enabled as boolean;
        toggleCalls.push({ path, enabled });
        const wasActive = rows.find((r) => r.path === path)?.active === true;
        rows = rows.map((r) => (r.path === path ? { ...r, enabled } : r));
        // Ratified OQ-HOME-8: the platform hands the stored marker to the
        // next enabled record; the window only re-reads that fact.
        if (!enabled && wasActive) {
          rows = rows.map((r) => (r.path === path ? { ...r, active: false } : r));
          const next = rows.find((r) => r.enabled);
          rows = rows.map((r) => (r === next ? { ...r, active: true } : r));
          sessionHome = next ? next.path : "";
        }
        return [...rows];
      }
      case "session_info": {
        const active = rows.find((r) => r.active);
        return {
          home: sessionHome,
          actor: "tester",
          opened_at: "2026-11-06T12:00:00Z",
          control_home: ACTIVE_HOME,
          degraded_from: active
            ? null
            : {
                code: "home_none_active",
                home: ACTIVE_HOME,
                message: "no active home — enable or add a home",
              },
        };
      }
      case "homes_activate":
        return {
          home: args.path,
          actor: "tester",
          opened_at: "2026-11-06T12:00:00Z",
          control_home: ACTIVE_HOME,
          degraded_from: null,
        };
      default:
        throw new Error(`unexpected command in HomesView actions test: ${cmd}`);
    }
  });
}

function inputOf(root: FakeNode): FakeNode {
  const input = elements(root).find((n) => n.type === "input");
  if (!input) throw new Error(`no <input> rendered: ${textOf(root)}`);
  return input;
}

async function click(node: FakeNode) {
  await (node.props.onClick as (e: unknown) => Promise<void>)({});
  await flush();
}

before(async () => {
  harness = await startHarness();
});

after(async () => {
  await harness?.close();
});

beforeEach(() => {
  wireHomes();
});

test("HomesView: add() регистрирует набранный путь через home.add и обновляет строки", async () => {
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");

  const input = inputOf(root);
  const button = buttonByLabel(root, "Добавить дом");
  assert.equal(button.props.disabled, true, "пустой путь — кнопка выключена");

  await typeInto(input, "/srv/billing/.devai");
  await flush();
  const enabled = buttonByLabel(root, "Добавить дом");
  assert.equal(enabled.props.disabled, false, "набранный путь включает кнопку");

  await click(buttonByLabel(root, "Добавить дом"));

  assert.deepEqual(addCalls, ["/srv/billing/.devai"], "home.add got the typed path");
  assert.ok(
    textOf(root).includes("/srv/billing/.devai"),
    `новая запись видна в таблице после перечитывания реестра: ${textOf(root)}`,
  );
  assert.equal(inputOf(root).value, "", "поле сброшено после регистрации");
});

test("HomesView: отказ платформы (путь без инициализированного стора) не роняет вид", async () => {
  addFailure = {
    code: "home_no_store",
    message: "/tmp/bogus carries no devai schema — it is not an initialised home",
  };
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");

  await typeInto(inputOf(root), "/tmp/bogus");
  await flush();
  await click(buttonByLabel(root, "Добавить дом"));

  assert.deepEqual(addCalls, ["/tmp/bogus"]);
  const text = textOf(root);
  assert.ok(text.includes("home_no_store"), `машинный код отказа виден: ${text}`);
  assert.ok(inputOf(root), "форма добавления осталась — UI пережил отказ");
});

test("HomesView: pick() регистрирует выбранную папку, отмена и сбой диалога — без home.add", async () => {
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");
  const globals = globalThis as { __DEVAI_TEST_DIALOG__?: unknown };

  globals.__DEVAI_TEST_DIALOG__ = PICKED_HOME;
  await click(buttonByLabel(root, "Выбрать папку…"));
  assert.deepEqual(addCalls, [PICKED_HOME], "диалог вернул путь — он ушёл в home.add");
  assert.ok(textOf(root).includes(PICKED_HOME));

  // Оператор отменил нативный диалог: штатный null, регистрации нет.
  globals.__DEVAI_TEST_DIALOG__ = null;
  await click(buttonByLabel(root, "Выбрать папку…"));
  assert.deepEqual(addCalls, [PICKED_HOME], "отмена не регистрирует ничего");

  // Вне Tauri диалог падает: честное уведомление, ручной ввод остаётся.
  globals.__DEVAI_TEST_DIALOG__ = new Error("no tauri window");
  await click(buttonByLabel(root, "Выбрать папку…"));
  assert.deepEqual(addCalls, [PICKED_HOME], "сбой диалога не регистрирует ничего");
  assert.match(textOf(root), /Нативный диалог недоступен/);
  delete globals.__DEVAI_TEST_DIALOG__;
});

test("HomesView: выключение АКТИВНОГО дома разрешено и маркер уходит дальше (OQ-HOME-8)", async () => {
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");

  // The active row carries the same switch-off affordance as any other
  // (before OQ-HOME-8 it was hidden behind a refusal).
  const switches = elements(root).filter(
    (n) => n.type === "button" && textOf(n).trim() === "Выключить",
  );
  assert.equal(switches.length, 2, textOf(root));

  await click(switches[0]);
  assert.deepEqual(toggleCalls, [{ path: ACTIVE_HOME, enabled: false }]);

  // The platform moved the marker to the next enabled home and the window
  // re-read the session; the registry table shows the handover state.
  const text = textOf(root);
  assert.match(text, /tripsales/, text);
  assert.match(text, /Включить/, "the switched-off row offers enabling back");
});

test("HomesView: когда выключены все дома — честная плашка «нет активного дома»", async () => {
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");

  // Switching both homes off leaves no active home at all; the registry
  // view must say so and offer both ways out — no silent pick.
  for (let i = 0; i < 2; i += 1) {
    const next = elements(root).find(
      (n) => n.type === "button" && textOf(n).trim() === "Выключить",
    );
    assert.ok(next, textOf(root));
    await click(next as FakeNode);
    await flush();
  }
  const text = textOf(root);
  assert.match(text, /Нет активного дома/, text);
  assert.match(text, /добавьте/, text);
});

test("HomesView: отказ homes.list показывает типизированный баннер и вид переживает его", async () => {
  listFailure = {
    code: "homes_registry_corrupt",
    message: "/home/op/.devai/homes.json is not a readable homes registry",
  };
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");

  const text = textOf(root);
  assert.ok(
    text.includes("homes_registry_corrupt"),
    `баннер несёт машинный код отказа: ${text}`,
  );
  assert.ok(
    text.includes("is not a readable homes registry"),
    "и текст платформы, не выдуманный фронтом",
  );
  assert.ok(buttonByLabel(root, "Обновить"), "кнопка повтора осталась — UI не умер");

  // Отказ был транзиентным: повторное перечисление его снимает и рисует
  // записи — тем же обработчиком, что и первая загрузка.
  listFailure = null;
  await click(buttonByLabel(root, "Обновить"));
  const healed = textOf(root);
  assert.ok(!healed.includes("homes_registry_corrupt"), "ошибка снята: " + healed);
  assert.ok(healed.includes(TRIPSALES_HOME), `записи реестра на экране: ${healed}`);
  assert.equal(listCalls, 2, "первая загрузка + повтор");
});

test("HomesView: кнопка «Обновить» перечитывает реестр и показывает новые записи", async () => {
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root } = await mount(harness, view, {}, "/homes");
  assert.equal(listCalls, 1);
  assert.ok(!textOf(root).includes(PICKED_HOME));

  // Другая сессия зарегистрировала дом: обновление показывает его без
  // перезапуска процесса (REQ-HOME-03 AC1).
  rows.push(home(PICKED_HOME, { project: "picked" }));
  await click(buttonByLabel(root, "Обновить"));

  assert.equal(listCalls, 2, "«Обновить» — это вызов homes.list, не перерисовка кэша");
  assert.ok(textOf(root).includes(PICKED_HOME), "новая запись пришла из перечитывания");
});

test("ProjectsView: смена фильтра-дома перечитывает projects_all и меняет строки", async () => {
  const calls: (string | null)[] = [];
  installInvoke((cmd, raw) => {
    if (cmd !== "projects_all") {
      throw new Error(`unexpected command in the filter test: ${cmd}`);
    }
    const filter = ((raw ?? {}) as Args).home as string | null;
    calls.push(filter);
    if (filter === TRIPSALES_HOME) {
      return [block(TRIPSALES_HOME, [project("p-trip", "tripsales-only")])];
    }
    return [
      block(ACTIVE_HOME, [project("p-devai", "devai-only")]),
      block(TRIPSALES_HOME, [project("p-trip", "tripsales-only")]),
    ];
  });
  const view = await loadView(harness, "/src/views/ProjectsView.vue");
  const { root } = await mount(harness, view, {}, "/projects");
  await flush();

  const select = elements(root).find((n) => n.type === "select");
  if (!select) throw new Error(`no filter select: ${textOf(root)}`);
  assert.deepEqual(calls, [null], "первый рендер — все включённые дома");
  assert.ok(textOf(root).includes("devai-only"));

  await (select.props.onChange as (e: unknown) => Promise<void>)({
    target: { value: TRIPSALES_HOME },
  });
  await flush();

  assert.deepEqual(calls, [null, TRIPSALES_HOME], "home-фильтр ушёл в API");
  const text = textOf(root);
  assert.ok(text.includes("tripsales-only"));
  assert.ok(!text.includes("devai-only"), "строки прежнего дома больше не на экране");
  assert.equal(harness.router.currentRoute.value.query.home, TRIPSALES_HOME);
});
