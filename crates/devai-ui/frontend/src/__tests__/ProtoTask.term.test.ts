// Консоль прогона в карточке: тот же поток, что пишет роль. Проверяем, что
// вкладка появляется у шага с прогоном, показывает текст потока и что живой
// прогон ДОСМАТРИВАЕТСЯ (опрос), а завершённый — перестаёт опрашиваться.
import { mount, flushPromises } from "@vue/test-utils";
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

const mocks = vi.hoisted(() => ({
  taskProgress: vi.fn(),
  taskReport: vi.fn(async () => null),
  taskUsage: vi.fn(async () => null),
  evaluationRules: vi.fn(async () => null),
  taskSignature: vi.fn(async () => null),
  artifactsList: vi.fn(async (): Promise<any[]> => []),
  questionsList: vi.fn(async () => []),
  taskEvents: vi.fn(async () => []),
  execTranscript: vi.fn(),
  artifactContent: vi.fn(async () => ({ shape: "text", artifact_id: "a1", text: "{}" })),
  taskRestate: vi.fn(async () => ({})),
  taskRewind: vi.fn(async () => ({})),
  taskStart: vi.fn(async () => ({})),
  taskRun: vi.fn(async () => ({})),
  execCancel: vi.fn(async () => ({})),
}));

vi.mock("vue-router", () => ({
  useRoute: () => ({ params: { id: "01a1b085-0000-7000-8000-000000000001" } }),
  useRouter: () => ({ push: vi.fn() }),
}));

vi.mock("../api", () => ({ api: mocks, toUiError: (e: unknown) => e }));

import ProtoTask from "../proto/ProtoTask.vue";

const task = {
  id: "01a1b085-0000-7000-8000-000000000001", project_id: null,
  title: "Контроль токенов", business_description: "Постановка.", human_name: null,
  state: "IMPLEMENTATION", return_state: null, requirements_seq: 1,
  lock_version: 2, created_at: "2026-11-13T10:00:00Z", updated_at: "2026-11-13T10:00:00Z",
  display: "Контроль токенов",
};

const stages = [
  ["STATEMENT", "постановка", "done"], ["ANALYSIS", "анализ", "done"],
  ["REVIEW", "ревью", "done"], ["APPROVAL", "подпись", "done"],
  ["IMPLEMENTATION", "код", "current"], ["QA", "приёмка", "todo"],
].map(([state, label, status]) => ({ state, label, status, gate: false, status_word: "" }));

const exec = {
  id: "01a1c9e4-4da9-7269-9a41-4319060b5176",
  role_slug: "implementation-agent", role_version: 1, status: "RUNNING",
  started_at: "2026-11-15T10:00:00Z", finished_at: null,
};

const progress = {
  task, snapshot_at: "", pipeline: { stages },
  graph: {
    stages,
    nodes: [{
      state: "IMPLEMENTATION", kind: "role", role_slug: "implementation-agent",
      role_version: 1, executions: [exec], artifacts: [],
      input_contract: null, output_contract: null,
    }],
    edges: [], position: {}, rework: {}, running: [exec.id], waits: [],
    blocking_questions: 0, notes: [],
  },
  allowed_operations: [], running_executions: [], convergence: null,
  attempt_limit: null, agent_failure: null,
};

const live = {
  execution: exec.id, task: task.id, role: "implementation-agent",
  status: "RUNNING", running: true, started_at: exec.started_at, finished_at: null,
  stdout: "читаю файлы\nпишу патч\n", stderr: "", stdout_bytes: 27, stderr_bytes: 0,
  truncated: false, note: null,
};

async function openImpl(w: any) {
  // Шаги идут в порядке стадий: реализация — пятая (STATEMENT…IMPLEMENTATION).
  const step = w.findAll(".p-vstep")[4];
  // Текущий шаг уже раскрыт карточкой — клик его ЗАКРЫЛ бы.
  if (!step.classes().includes("open")) {
    await step.find(".p-vhead").trigger("click");
    await flushPromises();
  }
}

describe("консоль прогона", () => {
  beforeEach(() => {
    mocks.taskProgress.mockResolvedValue(progress);
    mocks.execTranscript.mockResolvedValue(live);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("у шага с прогоном есть вкладка «консоль», и она показывает поток роли", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await openImpl(w);

    const tab = w.findAll(".p-tab").find((t) => t.text() === "консоль");
    expect(tab, "вкладка консоли").toBeTruthy();
    await tab!.trigger("click");
    await flushPromises();

    expect(w.find(".p-term").text()).toContain("пишу патч");
    expect(w.find(".p-termhead").text()).toContain("implementation-agent");
  });

  it("живой прогон досматривается опросом, а завершённый перестаёт опрашиваться", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await openImpl(w);
    vi.useFakeTimers();
    await w.findAll(".p-tab").find((t) => t.text() === "консоль")!.trigger("click");
    await flushPromises();
    expect(mocks.execTranscript).toHaveBeenCalledTimes(1);

    // прогон ещё идёт — окно перечитывает поток
    await vi.advanceTimersByTimeAsync(1300);
    await flushPromises();
    expect(mocks.execTranscript).toHaveBeenCalledTimes(2);

    // прогон завершился: следующее чтение говорит «не идёт» — опрос встаёт
    mocks.execTranscript.mockResolvedValue({ ...live, running: false, status: "COMPLETED", finished_at: "2026-11-15T10:05:00Z" });
    await vi.advanceTimersByTimeAsync(1300);
    await flushPromises();
    const calls = mocks.execTranscript.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    expect(mocks.execTranscript).toHaveBeenCalledTimes(calls);
  });
});
