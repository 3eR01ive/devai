// Session state of the window (R2 quality return): the shell used to read
// `session_info` ONCE in `onMounted`, so switching the active home (or a
// degraded startup healing) left the status bar, and every view branching
// on `session.degraded_from`, showing a stale fact. This module is the one
// authority: whoever changes the session (`activate` returns the fresh
// SessionInfo, the shell refreshes on mount) publishes it here and the
// subscribers re-render.
//
// Pure TS on purpose — no `vue` import — so the node harness
// (`frontend/tests/session-store.test.ts`) executes the same code the
// window runs. `src/session.ts` binds the singleton to `api.sessionInfo`.

import type { SessionInfo } from "./types";

export type SessionListener = (session: SessionInfo | null) => void;

export interface SessionStore {
  /** Current value; null until the first successful load. */
  get(): SessionInfo | null;
  /** Publish a session the caller already has (e.g. `home.activate`). */
  set(session: SessionInfo): void;
  /** Re-read the session through the loader; rejects on loader failure. */
  refresh(): Promise<SessionInfo>;
  /** Subscribe; returns the unsubscribe function. */
  subscribe(listener: SessionListener): () => void;
}

/**
 * Create a session store over a loader (injectable: the app passes
 * `api.sessionInfo`, the tests pass a fake). A failed refresh keeps the
 * last good value — the status bar must not go blank because one call
 * failed, and the error is the caller's to render.
 */
export function createSessionStore(
  load: () => Promise<SessionInfo>,
): SessionStore {
  let current: SessionInfo | null = null;
  const listeners = new Set<SessionListener>();

  function publish(next: SessionInfo) {
    current = next;
    for (const listener of listeners) listener(current);
  }

  return {
    get: () => current,
    set: publish,
    async refresh() {
      const next = await load();
      publish(next);
      return next;
    },
    subscribe(listener) {
      listeners.add(listener);
      listener(current);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
