// The window's single session store instance — the shell (App.vue) and the
// homes view (after `home.activate`) publish through this same object, so a
// home switch re-renders every consumer without a reload (R2 quality
// return). Kept apart from `session-store.ts` because this file names the
// Tauri `invoke` transport and the pure module must stay importable by the
// node harness.

import { api } from "./api";
import { createSessionStore } from "./session-store";

export const sessionStore = createSessionStore(() => api.sessionInfo());
