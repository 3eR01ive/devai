// Задача, у которой прогон упал, обязана это ПОКАЗАТЬ: платформа записала
// причину (exec.get: класс и текст отказа; progress.agent_failure: словами о
// провале агента), окно раньше писало «этот прогон ничего не отдал» — правда
// без причины и без пути дальше.
import { mount, flushPromises } from "@vue/test-utils";
import { describe, expect, it, vi, beforeEach } from "vitest";

const mocks = vi.hoisted(() => ({
  taskProgress: vi.fn(),
  taskReport: vi.fn(async () => null),
  taskUsage: vi.fn(async () => null),
  evaluationRules: vi.fn(async () => null),
  taskSignature: vi.fn(async () => null),
  artifactsList: vi.fn(async (): Promise<any[]> => []),
  questionsList: vi.fn(async () => []),
  taskEvents: vi.fn(async () => []),
  taskDecisions: vi.fn(async (): Promise<any[]> => []),
  execGet: vi.fn(),
  execTranscript: vi.fn(),
  artifactContent: vi.fn(async () => ({ shape: "text", artifact_id: "a1", text: "{}" })),
  taskRestate: vi.fn(async () => ({})),
  taskRewind: vi.fn(async () => ({})),
  taskStart: vi.fn(async () => ({})),
  taskRun: vi.fn(async () => ({})),
  taskResume: vi.fn(async () => ({})),
  taskAccept: vi.fn(async () => ({})),
  taskReturn: vi.fn(async () => ({})),
  taskMerge: vi.fn(async () => ({ branch: "", commit: "", text: "" })),
  execCancel: vi.fn(async () => ({})),
}));

vi.mock("vue-router", () => ({
  useRoute: () => ({ params: { id: "01a1b851-0000-7000-8000-000000000001" } }),
  useRouter: () => ({ push: vi.fn() }),
}));
vi.mock("../api", () => ({ api: mocks, toUiError: (e: any) => ({ message: e?.message ?? String(e) }) }));

import ProtoTask from "../proto/ProtoTask.vue";

const task = {
  id: "01a1b851-0000-7000-8000-000000000001", project_id: null,
  title: "Контроль токенов", business_description: "Постановка.", human_name: null,
  state: "BLOCKED", return_state: null, requirements_seq: 1, lock_version: 9,
  created_at: "2026-11-15T10:00:00Z", updated_at: "2026-11-15T10:00:00Z", display: "Контроль токенов",
};

const stages = [
  ["STATEMENT", "постановка", "done"], ["ANALYSIS", "анализ", "done"],
  ["REVIEW", "ревью", "done"], ["APPROVAL", "подпись", "done"],
  ["IMPLEMENTATION", "реализация", "current"], ["QA", "приёмка", "todo"],
].map(([state, label, status]) => ({ state, label, status, gate: false, status_word: "" }));

const failedExec = {
  id: "01a1e06d-130f-7522-8168-b6456445f81d", role_slug: "implementation-agent",
  role_version: 1, status: "FAILED", attempt: 1,
  started_at: "2026-11-15T18:16:47Z", finished_at: "2026-11-15T18:32:18Z",
};

const progress = {
  task, snapshot_at: "", pipeline: { stages },
  graph: {
    stages,
    nodes: [{ state: "IMPLEMENTATION", kind: "role", role_slug: "implementation-agent", role_version: 1, executions: [failedExec], artifacts: [] }],
    edges: [], position: {}, rework: {}, running: [], waits: [], blocking_questions: 0, notes: [],
  },
  allowed_operations: [], running_executions: [], convergence: null,
  attempt_limit: { role: "implementation-agent", attempts: 0, limit: 5, line: "попыток без прогресса: 0 из 5" },
  agent_failure: {
    agent: "hermes", class: "provider_rate", class_word: "провайдер ограничил темп или квоту",
    action: "снизить темп или повторить позже", series_count: 1, threshold: 3, window_count: 1,
    total_failures: 10, first_at: "2026-11-15T18:32:18Z", last_at: "2026-11-15T18:32:18Z",
    parked: false, exit_code: 2, duration_ms: 931277, tail: "…(усечено) run out of iteration budget",
  },
};

const row = {
  id: failedExec.id, task_id: task.id, role_slug: "implementation-agent", role_version: 1,
  status: "FAILED", summary: null, failure_class: "provider", attempt: 1,
  error: "`hermes` exited with exit status: 2\nstderr: (empty)\nstdout:\nI've run out of iteration budget mid-implementation.",
  started_at: failedExec.started_at, finished_at: failedExec.finished_at,
};

describe("упавший прогон виден", () => {
  beforeEach(() => {
    mocks.taskProgress.mockResolvedValue(progress);
    mocks.execGet.mockResolvedValue(row);
    mocks.execTranscript.mockResolvedValue({
      execution: failedExec.id, task: task.id, role: "implementation-agent", status: "FAILED",
      running: false, started_at: row.started_at, finished_at: row.finished_at,
      stdout: "I've run out of iteration budget", stderr: "", stdout_bytes: 30, stderr_bytes: 0,
      truncated: false, note: null,
    });
  });

  it("отказ — вкладка ВНУТРИ шага, без баннера поверх карточки", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await flushPromises();

    // задачи-wide баннера нет: провал принадлежит шагу
    expect(w.find(".p-alert").exists()).toBe(false);

    const step = w.findAll(".p-vstep")[4];
    if (!step.classes().includes("open")) await step.find(".p-vhead").trigger("click");
    await flushPromises();
    await flushPromises();

    // на самом шаге видно, что он упал
    expect(w.findAll(".p-failchip").length).toBeGreaterThan(0);
    const tabs = w.findAll(".p-tab").map((t) => t.text());
    expect(tabs[0]).toBe("отказ");
    expect(tabs).toContain("консоль");

    // и во вкладке — причина словами платформы
    const fail = w.find(".rf-t");
    expect(fail.exists()).toBe(true);
    expect(fail.text()).toContain("out of iteration budget");
    expect(w.find(".rf-h").text()).toContain("провайдер");
  });

  it("мемо прогона не врёт «ничего не отдал», когда прогон упал", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await flushPromises();
    const step = w.findAll(".p-vstep")[4];
    if (!step.classes().includes("open")) await step.find(".p-vhead").trigger("click");
    await flushPromises();
    await flushPromises();

    const memo = w.find(".p-runmemo");
    expect(memo.text()).toContain("прогон упал");
    expect(memo.text()).not.toContain("ничего не отдал");
  });
});
