// REQ-LV-05/06 на поверхности «Активные»: строка живой таблицы несёт ТОТ ЖЕ
// факт живости и ТУ ЖЕ фразу платформы, что карточка и `devai task active`.
// Окно ничего не собирает: `owner_liveness` — токен ответа, `liveness_line` —
// готовая фраза (ревью R2, ts/coverage-static: ячейка живости рендерилась, но
// не была покрыта ни одним тестом).
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import LiveView from "../views/LiveView.vue";
import { LivenessToken } from "../states";

const mocks = vi.hoisted(() => ({ taskActive: vi.fn() }));

vi.mock("../api", () => ({
  api: mocks,
  toUiError: (e: unknown) => e,
}));

const stubs = { RouterLink: { template: "<a><slot /></a>" } };

function row(liveness: string, line: string, overrides: Record<string, unknown> = {}) {
  return {
    id: "e1",
    task_id: "t1",
    role: "implementation-agent@1",
    role_slug: "implementation-agent",
    role_version: 1,
    agent_name: "mock",
    started_at: "2026-11-08T10:00:00Z",
    elapsed_seconds: 90,
    state: "IMPLEMENTATION",
    owner_pid: 4242,
    owner_host: "host",
    owner_liveness: liveness,
    liveness_line: line,
    branch: null,
    worktree: null,
    note: null,
    stale: false,
    stale_terminal_task: false,
    stale_dead_owner: false,
    ...overrides,
  };
}

// Один rundown на таблицу: чипы состояний/живости печатаются токенами, а
// фраза вердикта — колонкой «Живость» рядом с чипом.
async function mountWith(rows: ReturnType<typeof row>[]) {
  mocks.taskActive.mockResolvedValue({ snapshot_at: "2026-11-08T10:00:00Z", rows });
  const w = mount(LiveView, { global: { stubs } });
  await flushPromises();
  return w;
}

function livenessCell(w: ReturnType<typeof mount>, role: string) {
  const live = w.findAll("tr").find((tr) => tr.text().includes(role));
  expect(live, `строка роли ${role} не отрендерилась`).toBeDefined();
  const cell = live!.findAll("td").find((td) => td.attributes("data-label") === "Живость");
  expect(cell, "ячейка живости не отрендерилась").toBeDefined();
  return cell!;
}

beforeEach(() => {
  for (const fn of Object.values(mocks)) fn.mockReset();
});

describe("LiveView · живость в «Активных»", () => {
  it("prints the death verdict the platform gives, not a re-worded state", async () => {
    const w = await mountWith([
      row(LivenessToken.Dead, "исполнение мертво (можно перезапустить)"),
    ]);
    const cell = livenessCell(w, "implementation-agent");
    expect(cell.text()).toContain(LivenessToken.Dead);
    expect(cell.text()).toContain("исполнение мертво (можно перезапустить)");
    w.unmount();
  });

  it("keeps «проверить нельзя» its own word, never death", async () => {
    const w = await mountWith([row(LivenessToken.Unknown, "живость проверить нельзя")]);
    const cell = livenessCell(w, "implementation-agent");
    expect(cell.text()).toContain(LivenessToken.Unknown);
    expect(cell.text()).toContain("живость проверить нельзя");
    expect(cell.text()).not.toContain("мертво");
    w.unmount();
  });

  it("shows the live verdict for a live owner", async () => {
    const w = await mountWith([row(LivenessToken.Alive, "процесс жив")]);
    const cell = livenessCell(w, "implementation-agent");
    expect(cell.text()).toContain(LivenessToken.Alive);
    expect(cell.text()).toContain("процесс жив");
    w.unmount();
  });
});
