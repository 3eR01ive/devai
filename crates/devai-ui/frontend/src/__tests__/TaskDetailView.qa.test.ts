// Rework R3: REQ-SIGN-08 «честная пустота» — панель «Отчёт качества» при
// типизированном отказе `not_found` (QA ещё не прогонялся) обязана писать
// нейтральный текстом, без красной плашки, кода и платформенного сообщения;
// любой другой код остаётся ошибкой. Тест монтирует реальный
// TaskDetailView.vue с замоканным IPC — та же ветка шаблона, что в окне.
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import TaskDetailView from "../views/TaskDetailView.vue";
import { task } from "./fixtures";

const mocks = vi.hoisted(() => ({
  taskStatus: vi.fn(),
  taskProgress: vi.fn(),
  taskDependencies: vi.fn(),
  artifactsList: vi.fn(),
  taskDecisions: vi.fn(),
  executionsList: vi.fn(),
  taskActive: vi.fn(),
  taskEvents: vi.fn(),
  taskReport: vi.fn(),
  taskWorkspaceChanges: vi.fn(),
  taskSignature: vi.fn(),
  sessionInfo: vi.fn(),
  artifactContent: vi.fn(),
  taskApprove: vi.fn(),
  taskReject: vi.fn(),
}));

vi.mock("../api", () => ({
  api: mocks,
  toUiError: (e: unknown) => e,
}));

vi.mock("vue-router", () => ({
  useRoute: () => ({ query: {} }),
  useRouter: () => ({ push: vi.fn() }),
}));

vi.mock("../activation", () => ({ activateHome: vi.fn() }));

const stubs = { RouterLink: { template: "<a><slot /></a>" } };

function theTask() {
  return task("t1", "DRAFT", "карточка задачи");
}

function wire(taskReport: () => Promise<unknown>) {
  mocks.taskStatus.mockResolvedValue({
    task: theTask(),
    allowed_operations: [],
    current_artifacts: [],
    open_questions: [],
  });
  mocks.taskProgress.mockResolvedValue({
    task: theTask(),
    snapshot_at: "2026-11-08T10:00:00Z",
    pipeline: {
      order: ["ANALYSIS"],
      current: "DRAFT",
      position: null,
      position_index: null,
      off_spine: true,
      terminal: false,
    },
    allowed_operations: [],
    running_executions: [],
  });
  mocks.taskDependencies.mockResolvedValue({
    task: theTask(),
    outgoing: [],
    incoming: [],
  });
  mocks.artifactsList.mockResolvedValue([]);
  mocks.taskDecisions.mockResolvedValue([]);
  mocks.executionsList.mockResolvedValue([]);
  mocks.taskActive.mockResolvedValue({ snapshot_at: "2026-11-08T10:00:00Z", rows: [] });
  mocks.taskEvents.mockResolvedValue([]);
  mocks.taskReport.mockImplementation(taskReport);
  mocks.taskWorkspaceChanges.mockResolvedValue(null);
  mocks.taskSignature.mockResolvedValue(null);
  mocks.sessionInfo.mockResolvedValue({ home: "/home" });
}

async function mountView() {
  const w = mount(TaskDetailView, {
    props: { id: "t1" },
    global: { stubs },
  });
  await flushPromises();
  return w;
}

function qualityPanel(w: ReturnType<typeof mount>) {
  const panel = w.findAll(".panel").find((p) => p.text().includes("Отчёт качества"));
  expect(panel, "панель «Отчёт качества» не отрендерилась").toBeDefined();
  return panel!;
}

beforeEach(() => {
  for (const fn of Object.values(mocks)) fn.mockReset();
});

describe("TaskDetailView · панель «Отчёт качества»", () => {
  it("prints «QA ещё не прогонялся» for not_found, without an errorbox", async () => {
    wire(() =>
      Promise.reject({
        code: "not_found",
        message: "QA has not run for this task",
      }),
    );
    const w = await mountView();
    const panel = qualityPanel(w);
    expect(panel.text()).toContain("QA ещё не прогонялся");
    expect(panel.text()).not.toContain("not_found");
    expect(panel.text()).not.toContain("QA has not run");
    expect(panel.find(".errorbox").exists()).toBe(false);
    w.unmount();
  });

  it("keeps the errorbox for any other code", async () => {
    wire(() =>
      Promise.reject({
        code: "io",
        message: "диск недоступен",
      }),
    );
    const w = await mountView();
    const panel = qualityPanel(w);
    expect(panel.find(".errorbox").exists()).toBe(true);
    expect(panel.text()).toContain("io");
    expect(panel.text()).toContain("диск недоступен");
    expect(panel.text()).not.toContain("QA ещё не прогонялся");
    w.unmount();
  });
});
