// R2 quality return: the shell used to read `session_info` once on mount,
// so a home switch (or a degraded startup healing) left every session
// consumer showing the launch-time fact. `session-store.ts` is the pure
// state the window subscribes to; these tests execute exactly that code —
// publish on activate, refresh on mount, unsubscribe on unmount — and
// prove the derived banner follows the switched home.

import assert from "node:assert/strict";
import { test } from "node:test";

import { degradedBanner } from "../src/homes-view.ts";
import { createSessionStore } from "../src/session-store.ts";
import type { SessionInfo } from "../src/types.ts";

function info(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    home: "/home/devai/.devai",
    actor: "tester",
    opened_at: "2026-11-06T12:00:00Z",
    control_home: "/home/.devai",
    degraded_from: null,
    ...over,
  };
}

test("store starts empty, refresh publishes the loaded session", async () => {
  const expected = info();
  const store = createSessionStore(async () => expected);
  assert.equal(store.get(), null);

  const seen: (SessionInfo | null)[] = [];
  store.subscribe((s) => seen.push(s));
  // A late subscriber learns the current value immediately.
  assert.deepEqual(seen, [null]);

  const loaded = await store.refresh();
  assert.equal(loaded, expected);
  assert.equal(store.get(), expected);
  assert.deepEqual(seen, [null, expected]);
});

test("activating another home republishes it to every subscriber", async () => {
  // The window flow: shell refreshed on mount, then home.activate returns
  // the fresh SessionInfo and HomesView publishes it.
  const store = createSessionStore(async () => info({ home: "/a/.devai" }));
  await store.refresh();

  const rendered: string[] = [];
  store.subscribe((s) => rendered.push(s?.home ?? "none"));

  const switched = info({ home: "/srv/tripsales/services/.devai" });
  store.set(switched);

  assert.equal(store.get()?.home, "/srv/tripsales/services/.devai");
  assert.deepEqual(rendered, ["/a/.devai", "/srv/tripsales/services/.devai"]);
});

test("switching the home clears the degraded banner (derived, not cached)", async () => {
  const degraded = info({
    home: "/home/.devai",
    degraded_from: {
      code: "home_unavailable",
      home: "/srv/tripsales/.devai",
      message: "home /srv/tripsales/.devai is not on disk",
    },
  });
  const store = createSessionStore(async () => degraded);
  await store.refresh();
  assert.ok(degradedBanner(store.get()));

  store.set(info({ home: "/srv/tripsales/.devai" }));
  assert.equal(degradedBanner(store.get()), null);
});

test("a failed refresh keeps the last good session and rejects", async () => {
  let calls = 0;
  const store = createSessionStore(async () => {
    calls += 1;
    if (calls === 1) return info({ home: "/good/.devai" });
    throw new Error("ipc down");
  });
  await store.refresh();
  await assert.rejects(() => store.refresh(), /ipc down/);
  // The status bar does not go blank because one call failed.
  assert.equal(store.get()?.home, "/good/.devai");
});

test("unsubscribe stops delivery (unmount does not leak listeners)", async () => {
  const store = createSessionStore(async () => info());
  const seen: (string | null)[] = [];
  const stop = store.subscribe((s) => seen.push(s?.home ?? null));
  assert.equal(seen.length, 1);

  stop();
  store.set(info({ home: "/b/.devai" }));
  assert.equal(seen.length, 1);
});
