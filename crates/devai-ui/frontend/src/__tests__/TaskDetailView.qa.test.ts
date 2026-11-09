// Rework R3: REQ-SIGN-08 «честная пустота» — панель «Отчёт качества» при
// типизированном отказе `not_found` (QA ещё не прогонялся) обязана писать
// нейтральный текстом, без красной плашки, кода и платформенного сообщения;
// любой другой код остаётся ошибкой. Тест монтирует реальный
// TaskDetailView.vue с замоканным IPC — та же ветка шаблона, что в окне.
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import TaskDetailView from "../views/TaskDetailView.vue";
import { task } from "./fixtures";
import { HOLD_MS } from "../signature_view";

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
  taskAccept: vi.fn(),
  taskReturn: vi.fn(),
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
    quality_gate: null,
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

  it("accepts from the window through the same operation, with the visible rationale", async () => {
    wire(() => Promise.resolve(qaReport()));
    const qa = task("t1", "QA", "гейт качества");
    mocks.taskStatus.mockResolvedValue({
      task: qa,
      allowed_operations: ["accept", "return"],
      current_artifacts: [],
      open_questions: [],
      quality_gate: {
        audit_running: false,
        current_report: "art-1",
        open_blockers: [],
        may_accept: true,
        accept_denied: null,
        may_return: true,
        return_denied: null,
      },
    });
    mocks.taskAccept.mockResolvedValue({
      task: { ...qa, state: "ACCEPTED" },
      allowed_operations: [],
      current_artifacts: [],
      open_questions: [],
      quality_gate: null,
    });

    let frames: FrameRequestCallback[] = [];
    let now = 0;
    vi.spyOn(performance, "now").mockImplementation(() => now);
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      frames.push(cb);
      return frames.length;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {
      frames = [];
    });

    const w = await mountView();
    await w.find("textarea").setValue("принято человеком");
    await w.find("button.hold.approve").trigger("pointerdown");
    now = HOLD_MS + 10;
    const pending = frames;
    frames = [];
    for (const frame of pending) frame(0);
    await flushPromises();

    expect(mocks.taskAccept).toHaveBeenCalledTimes(1);
    expect(mocks.taskAccept).toHaveBeenCalledWith("t1", "принято человеком");
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    w.unmount();
  });
});

function qaReport() {
  return {
    task: "t1",
    artifact: "01a1b2aa-1111-700a-aaaa-111111111111",
    seq: 1,
    contract: "quality-report@5",
    created_at: "2026-11-09T10:00:00Z",
    execution: null,
    status: "CURRENT",
    report: {
      human_view: {
        version: 1,
        lang: "ru",
        headline: {
          verdict: "ПРОШЁЛ",
          machine_verdict: "PASS",
          score: 1,
          theta: 0.9,
          numbers: "S=1.000/1.00, порог 0.900",
          basis: "замечаний нет",
          counts: { blocks: 0, nuances: 0, anchors: 0, unchecked: 0 },
        },
        groups: [],
        blocked_weight: 0,
        max_blocked_weight: 0,
      },
    },
    report_text: null,
    source: "human_form",
    freshness: null,
  };
}
