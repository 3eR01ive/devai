// Новый список задач: закрытые дела не выброшены — по умолчанию свернуты одной
// группой, разворачиваются кнопкой в шапке; идущие сейчас — отдельной группой.
import { mount } from "@vue/test-utils";
import { flushPromises } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  tasksList: vi.fn(),
  taskProgress: vi.fn(),
  taskUsage: vi.fn(),
}));

vi.mock("../api", () => ({
  api: {
    tasksList: mocks.tasksList,
    taskProgress: mocks.taskProgress,
    taskUsage: mocks.taskUsage,
  },
  toUiError: (e: unknown) => e,
}));

import ProtoTasks from "../proto/ProtoTasks.vue";

const task = (id: string, state: string, title: string) => ({
  id, project_id: "p", title, business_description: "", human_name: null,
  state, return_state: null, requirements_seq: 0, lock_version: 1,
  created_at: "2026-11-14T10:00:00Z", updated_at: "2026-11-14T10:00:00Z",
  display: title,
});

const progress = (running: number) => ({
  task: task("x", "ANALYSIS", "x"),
  snapshot_at: "", pipeline: {}, graph: { stages: [], nodes: [], edges: [], position: {}, rework: {}, running: [], waits: [], blocking_questions: 0, notes: [] },
  allowed_operations: [],
  running_executions: Array.from({ length: running }, () => ({ id: "e", elapsed_seconds: 60 })),
  convergence: null, attempt_limit: null, agent_failure: null,
});

const stub = { RouterLink: { template: "<a><slot /></a>" } };

afterEach(() => {
  mocks.tasksList.mockReset();
  mocks.taskProgress.mockReset();
  mocks.taskUsage.mockReset();
});

describe("ProtoTasks", () => {
  it("закрытые свернуты по умолчанию и раскрываются кнопкой", async () => {
    mocks.tasksList.mockResolvedValue([
      task("t1", "ANALYSIS", "живая"),
      task("t2", "ACCEPTED", "принятая"),
    ]);
    mocks.taskProgress.mockResolvedValue(progress(0));
    mocks.taskUsage.mockResolvedValue({ total: 0, cost: 0, runs: 0, measured: 0 });

    const w = mount(ProtoTasks, { global: { stubs: stub } });
    await flushPromises();
    await flushPromises();

    expect(w.text()).not.toContain("принятая");
    const toggle = w.find(".p-toggle");
    expect(toggle.text()).toContain("закрытые · 1");

    await toggle.trigger("click");
    await flushPromises();
    await flushPromises();

    expect(w.text()).toContain("принятая");
    expect(w.find(".p-toggle").text()).toContain("скрыть закрытые");
  });

  it("идущее дело стоит группой «Идёт сейчас», а не дважды", async () => {
    mocks.tasksList.mockResolvedValue([task("t1", "ANALYSIS", "считает")]);
    mocks.taskProgress.mockResolvedValue(progress(1));
    mocks.taskUsage.mockResolvedValue({ total: 0, cost: 0, runs: 0, measured: 0 });

    const w = mount(ProtoTasks, { global: { stubs: stub } });
    await flushPromises();
    await flushPromises();

    const rows = w.findAll(".p-grp");
    const titles = rows.map((r) => r.find(".t").text());
    expect(titles[0]).toContain("Идёт сейчас");
    // одна и та же задача не появляется второй раз в стадийной группе
    expect(w.findAll(".p-row")).toHaveLength(1);
  });
});
