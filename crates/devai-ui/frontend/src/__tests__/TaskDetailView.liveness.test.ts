// REQ-LV-05: индикатор работы агента на карточке задачи. Окно печатает
// ГОТОВУЮ фразу платформы (`liveness_line` строки `exec.list`; ту же фразу
// несут task.status и task.progress) и не собирает вердикт само; при
// доказанной смерти рядом стоит явное действие, при «проверить нельзя» —
// честное слово, а не «работает». Отказ периодического перечитывания не
// выдаётся за свежий факт: карточка говорит «данные устарели».
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import TaskDetailView from "../views/TaskDetailView.vue";
import { pollMs } from "../settings";
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

function running(liveness: string, line: string) {
  return {
    id: "e1",
    role: "implementation-agent@1",
    role_slug: "implementation-agent",
    role_version: 1,
    agent_name: "mock",
    started_at: "2026-11-08T10:00:00Z",
    elapsed_seconds: 90,
    owner_pid: 4242,
    owner_host: "host",
    liveness,
    liveness_line: line,
  };
}

// Строка `exec.list` для одной RUNNING-записи: факт живости приходит в самой
// строке (ExecutionView), отдельного чтения на строку нет.
function execRows(rows: ReturnType<typeof running>[]) {
  return rows.map((r) => ({
    id: r.id,
    task_id: "t1",
    role_slug: r.role_slug,
    role_version: r.role_version,
    agent_name: r.agent_name,
    status: "RUNNING",
    inputs: [],
    summary: null,
    error: null,
    started_at: r.started_at,
    finished_at: null,
    liveness: r.liveness,
    liveness_line: r.liveness_line,
  }));
}

function wire(rows: ReturnType<typeof running>[]) {
  const theTask = task("t1", "IMPLEMENTATION", "карточка задачи");
  mocks.taskStatus.mockResolvedValue({
    task: theTask,
    allowed_operations: [],
    current_artifacts: [],
    open_questions: [],
    quality_gate: null,
    running_executions: rows,
  });
  mocks.taskProgress.mockResolvedValue({
    task: theTask,
    snapshot_at: "2026-11-08T10:00:00Z",
    pipeline: {
      order: ["ANALYSIS", "IMPLEMENTATION"],
      current: "IMPLEMENTATION",
      position: "IMPLEMENTATION",
      position_index: 1,
      off_spine: false,
      terminal: false,
    },
    allowed_operations: [],
    running_executions: rows,
    convergence: null,
    agent_failure: null,
  });
  mocks.taskDependencies.mockResolvedValue({ task: theTask, outgoing: [], incoming: [] });
  mocks.artifactsList.mockResolvedValue([]);
  mocks.taskDecisions.mockResolvedValue([]);
  // Индикатор читает ТУ ЖЕ строку `exec.list`, что и таблица исполнений:
  // факт живости приезжает в строке, отдельного чтения нет.
  mocks.executionsList.mockResolvedValue(execRows(rows));
  mocks.taskActive.mockResolvedValue({ snapshot_at: "2026-11-08T10:00:00Z", rows: [] });
  mocks.taskEvents.mockResolvedValue([]);
  mocks.taskReport.mockRejectedValue({ code: "not_found", message: "QA has not run" });
  mocks.taskWorkspaceChanges.mockResolvedValue(null);
  mocks.taskSignature.mockResolvedValue(null);
  mocks.sessionInfo.mockResolvedValue({ home: "/home" });
}

async function mountView() {
  const w = mount(TaskDetailView, { props: { id: "t1" }, global: { stubs } });
  await flushPromises();
  return w;
}

function indicator(w: ReturnType<typeof mount>) {
  const chip = w.find(".chip-liveness");
  expect(chip.exists(), "индикатор работы не отрендерился").toBe(true);
  return chip.text();
}

beforeEach(() => {
  for (const fn of Object.values(mocks)) fn.mockReset();
});

describe("TaskDetailView · индикатор работы агента", () => {
  it("prints the platform's own sentence for a live execution", async () => {
    wire([running("alive", "процесс жив")]);
    const w = await mountView();
    const text = indicator(w);
    expect(text).toContain("implementation-agent@1: процесс жив");
    // Живое исполнение не предлагает перезапуск.
    expect(w.text()).not.toContain("devai exec reap");
  });

  it("names the death and an explicit action when the owner is gone", async () => {
    wire([running("dead", "исполнение мертво (можно перезапустить)")]);
    const w = await mountView();
    const text = indicator(w);
    expect(text).toContain("исполнение мертво");
    expect(w.text()).toContain("devai exec reap");
    expect(w.text()).toContain("devai task run");
  });

  it("never claims «работает» for an unverifiable owner", async () => {
    wire([running("unknown", "живость проверить нельзя")]);
    const w = await mountView();
    const text = indicator(w);
    expect(text).toContain("живость проверить нельзя");
    expect(text).not.toContain("работает");
  });

  it("keeps no indicator when the task has no running execution", async () => {
    wire([]);
    const w = await mountView();
    expect(w.find(".chip-liveness").exists()).toBe(false);
  });

  // R2 (rl/error-handling): отказ периодического перечитывания не должен
  // выдавать старый факт за текущий — снимок остаётся, но помечен устаревшим.
  it("marks the snapshot stale when the periodic re-read fails", async () => {
    // Подменяем только интервал: flushPromises ждёт setTimeout, и фейковый
    // таймер на нём же подвесил бы сам тест.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const rows = [running("alive", "процесс жив")];
      wire(rows);
      // Первое чтение (loadAll) удалось, периодическое — отказ IPC/стора.
      mocks.executionsList
        .mockResolvedValueOnce(execRows(rows))
        .mockRejectedValueOnce({ code: "store", message: "store gone" });

      const w = mount(TaskDetailView, { props: { id: "t1" }, global: { stubs } });
      await flushPromises();
      expect(indicator(w)).toContain("процесс жив");
      expect(w.text()).not.toContain("данные устарели");

      await vi.advanceTimersByTimeAsync(pollMs());
      await flushPromises();

      expect(w.text()).toContain("данные устарели");
      expect(w.text()).toContain("последнее успешное чтение");
      // Последний факт не стёрт, но больше не подаётся как свежий.
      expect(indicator(w)).toContain("процесс жив");

      // Успешное перечитывание снимает метку устаревания.
      mocks.executionsList.mockResolvedValueOnce(execRows(rows));
      await vi.advanceTimersByTimeAsync(pollMs());
      await flushPromises();
      expect(w.text()).not.toContain("данные устарели");
    } finally {
      vi.useRealTimers();
    }
  });
});
