// Frontend harness of the homes cycle (R2 quality return): the branching
// logic the four views render, executed for real by the Node test runner
// (no browser, no extra dependency — `node --test` with native type
// stripping). The SFC templates call exactly these functions.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  HOME_CODES,
  RECENT_HEADING,
  blockIssueCode,
  blockIssueText,
  canActivate,
  degradedBanner,
  foreignHome,
  homeAttribution,
  homeFilterOptions,
  homeIssue,
  homeIssueKind,
  homeIssueLine,
  homeState,
  noActiveHomeNote,
  projectLabelNote,
  recentHomeLabel,
  recentHomes,
  rowsFor,
  schemaLabel,
  taskDetailLink,
  tasksEmptyText,
  tasksForProject,
} from "../src/homes-view.ts";
import type { HomeRecord, HomeRecordsView, Task } from "../src/types.ts";

function home(over: Partial<HomeRecord>): HomeRecord {
  return {
    path: "/p/.devai",
    project: "p",
    project_source: "store",
    project_error: null,
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

function block(
  home_path: string,
  over: Partial<HomeRecordsView<Task>> = {},
): HomeRecordsView<Task> {
  return {
    snapshot_at: "2026-11-06T12:00:00Z",
    home_path,
    home_project: home_path,
    is_active: false,
    available: true,
    version_note: null,
    error: null,
    records: [],
    ...over,
  };
}

function task(id: string, project_id: string | null, title = id): Task {
  return {
    id,
    project_id,
    title,
    business_description: "",
    human_name: null,
    // The finished signboard is produced by the backend (REQ-TN-03).
    display: title,
    state: "DRAFT",
    return_state: null,
    requirements_seq: 1,
    lock_version: 1,
    created_at: "2026-11-06T12:00:00Z",
    updated_at: "2026-11-06T12:00:00Z",
  };
}

test("registry row state: active > disabled > missing > included", () => {
  assert.equal(homeState(home({ active: true })), "ACTIVE");
  assert.equal(homeState(home({ enabled: false })), "DISABLED");
  assert.equal(homeState(home({ available: false })), "MISSING");
  assert.equal(homeState(home({})), "INCLUDED");
  // A contradictory record (active but switched off) is still the active
  // home: the writable home must never be rendered as an ordinary row.
  assert.equal(homeState(home({ active: true, enabled: false })), "ACTIVE");
});

test("activation affordance only for an included, readable, inactive home", () => {
  assert.equal(canActivate(home({})), true);
  assert.equal(canActivate(home({ active: true })), false);
  assert.equal(canActivate(home({ enabled: false })), false);
  assert.equal(canActivate(home({ available: false })), false);
  assert.equal(canActivate(home({ enabled: false, available: false })), false);
});

test("recent homes are ALL registry records, newest by added_at first", () => {
  // R1 quality return: the heading promises "recently added", so the
  // predicate is the added_at ordering over the whole registry — not a
  // second "is switched off" filter hiding enabled homes.
  const list = [
    home({ path: "/a", project: "a", active: true, added_at: "2026-11-01T09:00:00Z" }),
    home({ path: "/b", project: "b", enabled: false, added_at: "2026-11-05T09:00:00Z" }),
    home({ path: "/c", project: "c", added_at: "2026-11-03T09:00:00Z" }),
  ];
  assert.deepEqual(
    recentHomes(list).map((h) => h.path),
    ["/b", "/c", "/a"],
  );
  // The input order is not rearranged in place (render lists stay stable).
  assert.deepEqual(list.map((h) => h.path), ["/a", "/b", "/c"]);
  assert.deepEqual(recentHomes([]), []);
});

test("the recent heading describes the predicate that fills it", () => {
  // R1: the heading and the list must agree. `recentHomes` returns all
  // records (enabled too), so the heading is only truthful if it says so.
  const list = [
    home({ path: "/a", project: "a", active: true }),
    home({ path: "/b", project: "b", enabled: false }),
  ];
  assert.equal(recentHomes(list).length, list.length);
  assert.match(RECENT_HEADING, /все дома реестра/);
  assert.match(RECENT_HEADING, /свежие первыми/);
  assert.doesNotMatch(RECENT_HEADING, /^Выключенные/);
});

test("recent-list chip label carries the inclusion status, heading stays true", () => {
  // R1: a switched-off home is still "recently added" — but the label must
  // say it is off, otherwise the heading promises a home that is not in
  // the merged view.
  assert.equal(
    recentHomeLabel(home({ project: "tripsales", path: "/srv/tripsales/.devai" })),
    "tripsales · /srv/tripsales/.devai",
  );
  assert.equal(
    recentHomeLabel(
      home({ project: "tripsales", path: "/srv/tripsales/.devai", enabled: false }),
    ),
    "tripsales · /srv/tripsales/.devai · выключен",
  );
  // Every label of the recent list names its home: no unlabelled chip.
  const list = [
    home({ path: "/a", project: "a" }),
    home({ path: "/b", project: "b", enabled: false }),
  ];
  for (const h of recentHomes(list)) {
    assert.ok(recentHomeLabel(h).startsWith(`${h.project} · ${h.path}`));
  }
});

test("schema and issue cells stay honest for unknown / mismatched ranks", () => {
  assert.equal(schemaLabel(8), "v8");
  assert.equal(schemaLabel(7), "v7");
  assert.equal(schemaLabel(null), "—");
  assert.equal(homeIssue(home({})), null);
  assert.equal(homeIssueLine(home({ version_note: "behind" })), "behind");
  // A typed read error reaches the line with the platform hint appended —
  // the UI adds no wording of its own (R2 rel/error-context).
  assert.equal(
    homeIssueLine(
      home({
        read_error: {
          code: HOME_CODES.noStore,
          path: "/p/.devai",
          message: "no store",
          hint: "initialise the store (`devai init`) or add another path",
        },
      }),
    ),
    "no store — initialise the store (`devai init`) or add another path",
  );
  // The version note is the stronger fact when both are present.
  assert.equal(
    homeIssueLine(
      home({
        version_note: "ahead of",
        read_error: {
          code: HOME_CODES.unavailable,
          path: "/p/.devai",
          message: "io",
          hint: null,
        },
      }),
    ),
    "ahead of",
  );
});

test("typed home issues branch by CODE, never by the presence of text", () => {
  // R2 quality return: code + path + hint are wire data, so the views
  // decide by the code and an unknown code fails closed (still
  // unavailable), never silently "fine".
  assert.equal(homeIssueKind(null), null);
  assert.equal(
    homeIssueKind({ code: HOME_CODES.unavailable, path: "/p", message: "x", hint: null }),
    "unavailable",
  );
  assert.equal(
    homeIssueKind({ code: HOME_CODES.noStore, path: "/p", message: "x", hint: null }),
    "noStore",
  );
  assert.equal(
    homeIssueKind({ code: HOME_CODES.schemaVersion, path: "/p", message: "x", hint: null }),
    "schemaVersion",
  );
  assert.equal(
    homeIssueKind({ code: "future_code", path: "/p", message: "x", hint: null }),
    "unknown",
  );
  // The block banner shows the issue's own code, and text+hint as written.
  const issue = {
    code: HOME_CODES.noStore,
    path: "/p/.devai",
    message: "no store",
    hint: "initialise it",
  };
  assert.equal(blockIssueCode({ error: issue }), HOME_CODES.noStore);
  assert.equal(blockIssueText({ error: issue }), "no store — initialise it");
  // No cause at all: the banner falls back to the widest honest token
  // instead of inventing a specific one.
  assert.equal(blockIssueCode({ error: null }), HOME_CODES.unavailable);
  assert.equal(blockIssueText({ error: null }), null);
});

test("merged-view filter options follow the returned blocks, attribution intact", () => {
  const blocks = [
    block("/h1", { home_project: "devai", is_active: true }),
    block("/h2", { home_project: "tripsales" }),
  ];
  assert.deepEqual(homeFilterOptions(blocks), [
    { value: "/h1", label: "devai" },
    { value: "/h2", label: "tripsales" },
  ]);
  assert.equal(homeAttribution(blocks[0]), "devai · активный");
  assert.equal(homeAttribution(blocks[1]), "tripsales");
});

test("rows resolve inside their own block — same ids never merge homes", () => {
  const blocks = [
    block("/h1", { records: [task("t-1", "prj-1", "одинаковая")] }),
    block("/h2", { records: [task("t-1", "prj-1", "одинаковая")] }),
  ];
  assert.deepEqual(rowsFor(blocks, "/h1").map((t) => t.id), ["t-1"]);
  assert.deepEqual(rowsFor(blocks, "/h2").map((t) => t.id), ["t-1"]);
  assert.notEqual(rowsFor(blocks, "/h1"), rowsFor(blocks, "/h2"));
  assert.deepEqual(rowsFor(blocks, "/missing"), []);
});

test("project scoping filters per block and keeps every block", () => {
  const blocks = [
    block("/h1", {
      records: [task("a", "prj-1"), task("b", "prj-2")],
    }),
    block("/h2", {
      records: [task("c", "prj-1"), task("d", "prj-3")],
    }),
  ];
  const scoped = tasksForProject(blocks, "prj-1");
  assert.deepEqual(
    scoped.map((b) => [b.home_path, b.records.map((t) => t.id)]),
    [
      ["/h1", ["a"]],
      ["/h2", ["c"]],
    ],
  );
  // No project filter: the blocks are passed through untouched.
  assert.equal(tasksForProject(blocks, null), blocks);
  // An empty result is a real empty state per block, not a dropped block.
  const none = tasksForProject(blocks, "prj-x");
  assert.equal(none.length, 2);
  assert.deepEqual(none[0].records, []);
});

test("task deep link carries the source home, encoded", () => {
  const b = block("/home/guerrero/veeroute/tripsales/services/.devai");
  assert.equal(
    taskDetailLink(b, task("01a1", "prj")),
    "/task/01a1?home=%2Fhome%2Fguerrero%2Fveeroute%2Ftripsales%2Fservices%2F.devai",
  );
  assert.equal(
    taskDetailLink(block("/a b#c"), task("x", null)),
    "/task/x?home=%2Fa%20b%23c",
  );
});

test("empty-state wording distinguishes an empty project from an empty home", () => {
  assert.equal(
    tasksEmptyText(null),
    "В доме нет задач — пустое состояние, не ошибка",
  );
  assert.equal(
    tasksEmptyText("prj-1"),
    "У проекта в этом доме пока нет задач",
  );
});

test("foreign-home gate never follows another home silently", () => {
  // No home in the link: the active home is the implicit source.
  assert.equal(foreignHome(null, "/active"), null);
  assert.equal(foreignHome("/active", "/active"), null);
  // A different home is a gate, not a read and not a hidden switch.
  assert.equal(foreignHome("/other", "/active"), "/other");
  // Without a known active home the gate cannot prove a switch is needed,
  // so it does not invent one (session facts are the only source).
  assert.equal(foreignHome("/other", null), null);
});

test("degraded banner states the failed home, the cause and the fallback", () => {
  assert.equal(degradedBanner(null), null);
  assert.equal(
    degradedBanner({
      home: "/control",
      actor: "tester",
      opened_at: "2026-11-06T12:00:00Z",
      control_home: "/control",
      degraded_from: null,
    }),
    null,
  );
  const text = degradedBanner({
    home: "/control",
    actor: "tester",
    opened_at: "2026-11-06T12:00:00Z",
    control_home: "/control",
    degraded_from: {
      code: HOME_CODES.unavailable,
      home: "/srv/tripsales/.devai",
      message: "home /srv/tripsales/.devai is not on disk",
    },
  });
  assert.ok(text);
  assert.match(text, /\/srv\/tripsales\/\.devai/);
  assert.match(text, /is not on disk/);
  assert.match(text, /контрольный дом/);
  assert.match(text, /не изменена/);
});

test("project label provenance: an unreadable store is stated, an empty one is not an error (R3)", () => {
  // The store could not be read: the row says so and prints the cause.
  const unreadable = projectLabelNote({
    project_source: "unreadable",
    project_error: "the store of /p/.devai could not be opened: no devai.db",
  });
  assert.match(unreadable ?? "", /не прочитался/);
  assert.match(unreadable ?? "", /no devai\.db/, "причина платформы, не выдумка фронта");

  // The cause is optional in wording but the fact stays stated.
  assert.match(
    projectLabelNote({ project_source: "unreadable", project_error: null }) ?? "",
    /не прочитался/,
  );

  // The store answered with no projects: a deterministic path label, and
  // it is NOT presented as a read failure.
  const empty = projectLabelNote({ project_source: "path", project_error: null });
  assert.match(empty ?? "", /нет проектов/);
  assert.doesNotMatch(empty ?? "", /не прочитался/);

  // A real store-derived label needs no note; an older row stays silent.
  assert.equal(projectLabelNote({ project_source: "store", project_error: null }), null);
  assert.equal(projectLabelNote({ project_source: "unknown", project_error: null }), null);
});

test("no active home (ratified OQ-HOME-8): stated plainly, both ways out named", () => {
  // Empty registry is the valid pre-registration state, not this note.
  assert.equal(noActiveHomeNote([]), null);
  // A clean registry with an active home says nothing.
  assert.equal(noActiveHomeNote([{ active: true }, { active: false }]), null);
  // Every record switched off: the window states it and names the two ways
  // out (enable the record you name, or add one) — never a silent pick.
  const note = noActiveHomeNote([{ active: false }, { active: false }]);
  assert.ok(note);
  assert.match(note, /Нет активного дома/);
  assert.match(note, /Включить/);
  assert.match(note, /добавьте/);
});

test("the no-active session banner does not pretend a home failed", () => {
  const text = degradedBanner({
    home: "/home/op/.devai",
    actor: "tester",
    opened_at: "2026-11-06T12:00:00Z",
    control_home: "/home/op/.devai",
    degraded_from: {
      code: HOME_CODES.noneActive,
      home: "/home/op/.devai",
      message: "no active home — enable or add a home",
    },
  });
  assert.ok(text);
  assert.match(text, /Нет активного дома/);
  assert.match(text, /включите/);
  // The degraded-open wording (a stored home failed to open) is NOT reused:
  // nothing failed, the registry simply has no active record.
  assert.doesNotMatch(text, /не открылся/);
});
