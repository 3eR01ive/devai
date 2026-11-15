// Переключение прогонов: каждая засечка обязана срабатывать — и выбор
// прогона должен менять ДАННЫЕ шага. Прецедент: приёмка показывала прошлый
// отчёт, потому что тело уже лежало в кэше и привязка к выбранному прогону не
// обновлялась; плюс сама засечка была 7 пикселей, то есть по ней не попасть.
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
  artifactContent: vi.fn(),
  execTranscript: vi.fn(),
  taskAccept: vi.fn(async () => ({})),
  taskReturn: vi.fn(async () => ({})),
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
vi.mock("../api", () => ({ api: mocks, toUiError: (e: any) => ({ message: e?.message ?? String(e) }) }));

import ProtoTask from "../proto/ProtoTask.vue";

const task = {
  id: "01a1b085-0000-7000-8000-000000000001", project_id: null,
  title: "Прокрутка", business_description: "Постановка.", human_name: null,
  state: "QA", return_state: null, requirements_seq: 1, lock_version: 9,
  created_at: "2026-11-15T10:00:00Z", updated_at: "2026-11-15T10:00:00Z", display: "Прокрутка",
};

const stages = [
  ["STATEMENT", "постановка", "done"], ["ANALYSIS", "анализ", "done"],
  ["REVIEW", "ревью", "done"], ["APPROVAL", "подпись", "done"],
  ["IMPLEMENTATION", "реализация", "done"], ["QA", "приёмка", "current"],
].map(([state, label, status]) => ({ state, label, status, gate: state === "QA", status_word: "" }));

const execs = ["e1", "e2", "e3"].map((id, i) => ({
  id, role_slug: "quality-auditor", role_version: 1, status: "COMPLETED",
  started_at: `2026-11-15T1${i}:00:00Z`, finished_at: `2026-11-15T1${i}:10:00Z`,
}));

const artifacts = execs.map((e, i) => ({
  id: `qa-${e.id}`, task_id: task.id, kind: "QUALITY_REPORT", name: "качество",
  seq: i + 1, category: "artifact", status: i === 2 ? "CURRENT" : "STALE",
  sha256: "x", size: 10, media_type: "application/json", contract: null,
  created_by: "role", created_at: `2026-11-15T1${i}:00:00Z`,
  execution_id: e.id, display: `качество · v${i + 1}`,
}));

const progress = {
  task, snapshot_at: "", pipeline: { stages },
  graph: {
    stages,
    nodes: [{ state: "QA", kind: "role", role_slug: "quality-auditor", role_version: 1, executions: execs, artifacts }],
    edges: [], position: {}, rework: {}, running: [], waits: [], blocking_questions: 0, notes: [],
  },
  allowed_operations: [], running_executions: [], convergence: null, attempt_limit: null, agent_failure: null,
};

function reportWith(score: number) {
  return {
    human_view: { headline: { score, theta: 0.8 } },
    evaluation: { checks: [{ id: "cl/short-units", score, weight: 1 }] },
  };
}

describe("переключение прогонов", () => {
  beforeEach(() => {
    mocks.taskProgress.mockResolvedValue(progress);
    mocks.artifactsList.mockResolvedValue(artifacts);
    mocks.artifactContent.mockImplementation(async (artifactId: string) => {
      const i = artifacts.findIndex((a) => a.id === artifactId);
      return { shape: "text", artifact_id: artifactId, text: JSON.stringify(reportWith([0.5, 0.7, 0.9][i])) };
    });
  });

  it("каждая засечка меняет данные шага, а не только подсветку", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await flushPromises();

    const marks = w.findAll(".p-rdhit");
    expect(marks).toHaveLength(3);

    // ключевое: ПОСЛЕ первого переключения тело уже в кэше, и раньше второе
    // переключение молчало — число оставалось от прошлого прогона
    await marks[0].trigger("click");
    await flushPromises();
    expect(w.find(".p-hero .big").text()).toContain("50.0");

    await marks[1].trigger("click");
    await flushPromises();
    expect(w.find(".p-hero .big").text()).toContain("70.0");

    await marks[2].trigger("click");
    await flushPromises();
    expect(w.find(".p-hero .big").text()).toContain("90.0");
  });

  it("засечка — кнопка с областью нажатия больше самого штриха", async () => {
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await flushPromises();
    const hit = w.find(".p-rdhit");
    expect(hit.element.tagName).toBe("BUTTON");
    expect(hit.find(".p-rd").exists()).toBe(true);
  });
});
