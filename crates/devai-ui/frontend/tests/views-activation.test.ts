// R1/R6 quality return: компонентный тест ОБОИХ сайтов активации дома.
// До этого тестировались только чистые модули, поэтому пропуск «активация
// из чужого гейта не публикует сессию» (TaskDetailView) не был замечен.
// Здесь SFC монтируются по-настоящему, клик по кнопке исполняет шаблонный
// обработчик, IPC подменён mock-транспортом, а результат проверяется по
// общему стору сессии окна — то есть ровно по тому факту, который читает
// статус-бар (App.vue).

import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import {
  buttonByLabel,
  flush,
  installInvoke,
  loadView,
  mount,
  startHarness,
  textOf,
  type Harness,
} from "./support/component-harness.ts";

const ACTIVE_HOME = "/srv/devai/.devai";
const FOREIGN_HOME = "/srv/tripsales/services/.devai";

function session(home: string) {
  return {
    home,
    actor: "tester",
    opened_at: "2026-11-06T12:00:00Z",
    control_home: "/home/operator/.devai",
    degraded_from: null,
  };
}

let harness: Harness;
let activations: string[];

before(async () => {
  harness = await startHarness();
});

after(async () => {
  await harness?.close();
});

/** Fake IPC of the homes cycle: only the commands these views call. */
function wireHomesView() {
  activations = [];
  installInvoke((cmd, args) => {
    switch (cmd) {
      case "session_info":
        return session(ACTIVE_HOME);
      case "homes_list":
        return [
          {
            path: ACTIVE_HOME,
            project: "devai",
            added_at: "2026-11-01T10:00:00Z",
            enabled: true,
            active: true,
            available: true,
            schema_version: 8,
            version_note: null,
            read_error: null,
          },
          {
            path: FOREIGN_HOME,
            project: "tripsales",
            added_at: "2026-11-02T10:00:00Z",
            enabled: true,
            active: false,
            available: true,
            schema_version: 7,
            version_note: "stored schema v7 is behind this build (v8)",
            read_error: null,
          },
        ];
      case "homes_activate": {
        const path = (args as { path: string }).path;
        activations.push(path);
        return session(path);
      }
      default:
        throw new Error(`unexpected command in the homes view test: ${cmd}`);
    }
  });
}

test("HomesView: активация дома публикует свежую сессию окна", async () => {
  wireHomesView();
  const view = await loadView(harness, "/src/views/HomesView.vue");
  const { root, sessionStore } = await mount(harness, view, {}, "/homes");

  // A fresh store starts without a session; the view was mounted without
  // App.vue's refresh, so any published value came from the click itself.
  assert.equal(sessionStore.get(), null);

  const button = buttonByLabel(root, "Активировать");
  await (button.props.onClick as (e: unknown) => Promise<void>)({});
  await flush();

  assert.deepEqual(activations, [FOREIGN_HOME], "home.activate got the row path");
  assert.equal(
    sessionStore.get()?.home,
    FOREIGN_HOME,
    "the status bar source (session store) now names the activated home",
  );
});

test("TaskDetailView: активация из чужого гейта публикует сессию", async () => {
  // The exact regression R1 reported: this site called home.activate and
  // never published, so the status bar kept the launch-time home.
  installInvoke((cmd, args) => {
    if (cmd === "session_info") return session(ACTIVE_HOME);
    if (cmd === "homes_activate") {
      const path = (args as { path: string }).path;
      assert.equal(path, FOREIGN_HOME);
      return session(path);
    }
    throw new Error(`unexpected command in the gate test: ${cmd}`);
  });
  const view = await loadView(harness, "/src/views/TaskDetailView.vue");
  const { root, sessionStore } = await mount(
    harness,
    view,
    { id: "01a1ad7d-e489-7448-824b-a52df7a95bbb" },
    `/task/01a1ad7d-e489-7448-824b-a52df7a95bbb?home=${encodeURIComponent(FOREIGN_HOME)}`,
  );

  // The gate is rendered instead of the details (the home is not active).
  assert.match(textOf(root), /Запись из другого дома/);
  // Model the launch-time fact the status bar displays, then prove the
  // click replaces it (the store is per module graph, so an earlier test's
  // publication is overwritten explicitly).
  sessionStore.set({ home: ACTIVE_HOME });
  assert.equal(sessionStore.get()?.home, ACTIVE_HOME);

  const button = buttonByLabel(root, "Активировать этот дом");
  await (button.props.onClick as (e: unknown) => Promise<void>)({});
  await flush();

  assert.equal(
    sessionStore.get()?.home,
    FOREIGN_HOME,
    "the gate activation publishes the fresh session — no stale active home",
  );
});
