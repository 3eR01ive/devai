// R2 quality return: TasksView.vue and ProjectsView.vue were only covered
// through their pure helpers, so the TEMPLATES — the actual column carrying
// the home attribution (REQ-HOME-04 AC1) — were never mounted. Here both
// screens render through the component harness (the same Vue renderer the
// window uses) over the fake IPC, and the asserted fact is the operator's:
// the «Дом» column of a row shows BOTH the home's project name and its path
// as visible text, and same-named rows of two homes do not collapse.

import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import {
  flush,
  elements,
  installInvoke,
  loadView,
  mount,
  startHarness,
  textOf,
  type FakeNode,
  type Harness,
} from "./support/component-harness.ts";

const DEVAI_HOME = "/srv/devai/.devai";
const TRIPSALES_HOME = "/srv/tripsales/services/.devai";

const SNAPSHOT = "2026-11-06T12:00:00Z";

function task(id: string, project_id: string, title: string) {
  return {
    id,
    project_id,
    title,
    business_description: "",
    human_name: null,
    display: title,
    state: "DRAFT",
    return_state: null,
    requirements_seq: 1,
    lock_version: 1,
    created_at: SNAPSHOT,
    updated_at: SNAPSHOT,
  };
}

function taskBlock(home_path: string, home_project: string, records: unknown[]) {
  return {
    snapshot_at: SNAPSHOT,
    home_path,
    home_project,
    is_active: false,
    available: true,
    version_note: null,
    error: null,
    records,
  };
}

/** Visible text of every cell of the «Дом» column (header name included). */
function homeColumnCells(root: FakeNode): FakeNode[] {
  const cells: FakeNode[] = [];
  for (const table of elements(root).filter((n) => n.type === "table")) {
    let homeIndex = -1;
    for (const row of elements(table).filter((n) => n.type === "tr")) {
      const children = row.children.filter((n) => n.type === "th" || n.type === "td");
      if (children.some((n) => n.type === "th")) {
        homeIndex = children.findIndex((n) => textOf(n).trim() === "Дом");
        continue;
      }
      if (homeIndex >= 0 && children[homeIndex]) cells.push(children[homeIndex]);
    }
  }
  return cells;
}

let harness: Harness;

before(async () => {
  harness = await startHarness();
});

after(async () => {
  await harness?.close();
});

test("TasksView: колонка «Дом» несёт имя проекта и путь каждого дома", async () => {
  installInvoke((cmd) => {
    if (cmd === "tasks_all") {
      return [
        taskBlock(DEVAI_HOME, "devai", [
          task("01a1ad7d-e489-7448-824b-a52df7a95bbb", "p-devai", "Задача devai"),
        ]),
        taskBlock(TRIPSALES_HOME, "tripsales", [
          task("01a1b2ad-20ec-7211-8756-0840b6522d2b", "p-trip", "Задача tripsales"),
        ]),
      ];
    }
    throw new Error(`unexpected command in the tasks view test: ${cmd}`);
  });

  const view = await loadView(harness, "/src/views/TasksView.vue");
  const { root } = await mount(harness, view, {}, "/tasks");
  await flush();

  const cells = homeColumnCells(root);
  assert.equal(cells.length, 2, `row cells of the column: ${textOf(root)}`);

  const [devaiCell, tripsalesCell] = cells.map(textOf);
  assert.match(devaiCell, /devai/, "имя проекта-дома видно в колонке");
  assert.ok(devaiCell.includes(DEVAI_HOME), `путь дома виден: ${devaiCell}`);
  assert.match(tripsalesCell, /tripsales/);
  assert.ok(tripsalesCell.includes(TRIPSALES_HOME), `путь дома виден: ${tripsalesCell}`);

  // The same rows are not merged: both titles render, each under its home.
  const text = textOf(root);
  assert.ok(text.includes("Задача devai") && text.includes("Задача tripsales"));
});

test("ProjectsView: карточка проекта показывает дом (имя + путь) видимым текстом", async () => {
  installInvoke((cmd) => {
    if (cmd === "projects_all") {
      return [
        {
          ...taskBlock(DEVAI_HOME, "devai", []),
          records: [
            {
              id: "p-devai",
              name: "devai",
              description: "",
              repos: [],
              created_at: SNAPSHOT,
              updated_at: SNAPSHOT,
            },
          ],
        },
        {
          ...taskBlock(TRIPSALES_HOME, "tripsales", []),
          records: [
            {
              id: "p-trip",
              name: "tripsales",
              description: "",
              repos: [],
              created_at: SNAPSHOT,
              updated_at: SNAPSHOT,
            },
          ],
        },
      ];
    }
    throw new Error(`unexpected command in the projects view test: ${cmd}`);
  });

  const view = await loadView(harness, "/src/views/ProjectsView.vue");
  const { root } = await mount(harness, view, {}, "/projects");
  await flush();

  const text = textOf(root);
  const flat = text.replace(/\s+/g, " ");
  // Two project cards, both attributed: the name of the home and its path.
  assert.ok(flat.includes(`дом devai · ${DEVAI_HOME}`), text);
  assert.ok(flat.includes(`дом tripsales · ${TRIPSALES_HOME}`), text);
});

// REQ-NM-01 AC6: the lists lead with the human name — the hard id is the
// muted second position, never the heading. These two assertions pin the
// ORDER on the two lists whose rows carry both.
test("TasksView: имя задачи стоит раньше её идентификатора", async () => {
  installInvoke((cmd) => {
    if (cmd === "tasks_all") {
      return [
        taskBlock(DEVAI_HOME, "devai", [
          task("01a1ad7d-e489-7448-824b-a52df7a95bbb", "p-devai", "Задача devai"),
        ]),
      ];
    }
    throw new Error(`unexpected command in the tasks order test: ${cmd}`);
  });

  const view = await loadView(harness, "/src/views/TasksView.vue");
  const { root } = await mount(harness, view, {}, "/tasks");
  await flush();

  const flat = textOf(root).replace(/\s+/g, " ");
  const nameAt = flat.indexOf("Задача devai");
  const idAt = flat.indexOf("01a1ad7d");
  assert.ok(nameAt >= 0 && idAt >= 0, flat);
  assert.ok(nameAt < idAt, `имя должно идти первым: ${flat}`);
});

test("ProjectsView: имя проекта стоит раньше его идентификатора", async () => {
  installInvoke((cmd) => {
    if (cmd === "projects_all") {
      return [
        {
          ...taskBlock(DEVAI_HOME, "devai", []),
          // Имя — «название проекта», id — техническая вывеска рядом.
          records: [
            {
              id: "p-devai",
              name: "название проекта",
              description: "",
              repos: [],
              created_at: SNAPSHOT,
              updated_at: SNAPSHOT,
            },
          ],
        },
      ];
    }
    throw new Error(`unexpected command in the projects order test: ${cmd}`);
  });

  const view = await loadView(harness, "/src/views/ProjectsView.vue");
  const { root } = await mount(harness, view, {}, "/projects");
  await flush();

  const flat = textOf(root).replace(/\s+/g, " ");
  const nameAt = flat.indexOf("название проекта");
  const idAt = flat.indexOf("p-devai");
  assert.ok(nameAt >= 0 && idAt >= 0, flat);
  assert.ok(nameAt < idAt, `имя должно идти первым: ${flat}`);
});
