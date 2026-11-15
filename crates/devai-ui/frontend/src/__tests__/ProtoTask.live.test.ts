// Живое дерево в шаге реализации: пока роль работает, правки ещё не в коммите,
// и окно показывало пустой worktree. Панель читает рабочее дерево задачи и
// обновляет его, а файл открывается своим незакоммиченным диффом.
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
  taskDecisions: vi.fn(async (): Promise<any[]> => []),
  execGet: vi.fn(async () => ({ id: "e1", status: "RUNNING", error: null, summary: null })),
  execTranscript: vi.fn(),
  workspaceLive: vi.fn(),
  workspaceLiveDiff: vi.fn(),
  workspaceChanges: vi.fn(async () => null),
  artifactContent: vi.fn(async () => ({ shape: "text", artifact_id: "a1", text: "{}" })),
  taskRestate: vi.fn(async () => ({})),
  taskRewind: vi.fn(async () => ({})),
  taskStart: vi.fn(async () => ({})),
  taskRun: vi.fn(async () => ({})),
  taskResume: vi.fn(async () => ({})),
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
  state: "IMPLEMENTATION", return_state: null, requirements_seq: 1, lock_version: 9,
  created_at: "2026-11-15T10:00:00Z", updated_at: "2026-11-15T10:00:00Z", display: "Контроль токенов",
};

const stages = [
  ["STATEMENT", "постановка", "done"], ["ANALYSIS", "анализ", "done"],
  ["REVIEW", "ревью", "done"], ["APPROVAL", "подпись", "done"],
  ["IMPLEMENTATION", "реализация", "current"], ["QA", "приёмка", "todo"],
].map(([state, label, status]) => ({ state, label, status, gate: false, status_word: "" }));

const exec = {
  id: "e1", role_slug: "implementation-agent", role_version: 1, status: "RUNNING",
  started_at: "2026-11-15T22:11:00Z", finished_at: null,
};

const progress = {
  task, snapshot_at: "", pipeline: { stages },
  graph: {
    stages,
    nodes: [{ state: "IMPLEMENTATION", kind: "role", role_slug: "implementation-agent", role_version: 1, executions: [exec], artifacts: [] }],
    edges: [], position: {}, rework: {}, running: ["e1"], waits: [], blocking_questions: 0, notes: [],
  },
  allowed_operations: [], running_executions: [], convergence: null, attempt_limit: null, agent_failure: null,
};

describe("живое рабочее дерево", () => {
  beforeEach(() => {
    mocks.taskProgress.mockResolvedValue(progress);
    mocks.workspaceLive.mockResolvedValue({
      task: task.id, worktree: "/tmp/wt", branch: "rework/x", head: "abcdef1234567890",
      files: [{ path: "crates/a.rs", status: "M", added: 12, removed: 3 }],
      dirty: 1, truncated: false, running: true,
      note: null,
    });
    mocks.workspaceLiveDiff.mockResolvedValue({ path: "crates/a.rs", diff: "+привет\n", note: null });
  });
  afterEach(() => vi.clearAllMocks());

  it("показывает правки идущего прогона и открывает дифф файла", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await flushPromises();

    const step = w.findAll(".p-vstep")[4];
    if (!step.classes().includes("open")) await step.find(".p-vhead").trigger("click");
    await flushPromises();
    await w.findAll(".p-tab").find((t) => t.text() === "worktree")!.trigger("click");
    await flushPromises();

    expect(w.find(".p-live").text()).toContain("crates/a.rs");
    expect(w.find(".p-live").text()).toContain("+12");
    // пока прогон идёт — расход ещё не посчитан, окно это говорит
    expect(w.find(".p-live").text()).toContain("расход — после прогона");

    await w.find(".p-lvfile").trigger("click");
    await flushPromises();
    expect(mocks.workspaceLiveDiff).toHaveBeenCalledWith(task.id, "crates/a.rs");
    expect(w.find(".p-diff").text()).toContain("+привет");
  });
});
