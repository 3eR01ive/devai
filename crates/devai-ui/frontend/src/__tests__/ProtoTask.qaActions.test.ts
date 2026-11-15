// Ходы приёмки в карточке: «Принять задачу» и «На доработку» — оба удержанием
// на самой кнопке, и возврат уносит с собой ПУНКТЫ, выбранные человеком
// (таблица приёмки и есть выбор: что чинить), плюс его слова.
import { mount, flushPromises } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HOLD_MS } from "../signature_view";

const mocks = vi.hoisted(() => ({
  taskProgress: vi.fn(),
  taskReport: vi.fn(),
  taskUsage: vi.fn(async () => null),
  evaluationRules: vi.fn(async () => null),
  taskSignature: vi.fn(async () => null),
  artifactsList: vi.fn(async (): Promise<any[]> => []),
  questionsList: vi.fn(async () => []),
  taskEvents: vi.fn(async () => []),
  taskAccept: vi.fn(async () => ({})),
  taskReturn: vi.fn(async () => ({})),
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

const progress = {
  task, snapshot_at: "", pipeline: { stages },
  graph: { stages, nodes: [], edges: [], position: {}, rework: {}, running: [], waits: [], blocking_questions: 0, notes: [] },
  allowed_operations: [], running_executions: [], convergence: null, attempt_limit: null, agent_failure: null,
};

const report = {
  task: task.id, label: "качество · v1", seq: 1, freshness: "current",
  artifact: { id: "a1", kind: "QUALITY_REPORT", status: "CURRENT", seq: 1, display: "качество · v1" },
  contract: null,
  report: {
    human_view: { headline: { score: 0.96, theta: 0.8, blocked_max: 0.2, verdict: "приёмка" } },
    evaluation: {
      checks: [
        { id: "cl/style-consistency", score: 0.0, weight: 0.02, group: "cl", error_type: "MECHANICAL_FAIL", note: "почему" },
        { id: "cl/short-units", score: 0.7, weight: 0.02, group: "cl" },
        { id: "ts/coverage-static", score: 1.0, weight: 0.02, group: "ts" },
      ],
    },
  },
};

let frames: FrameRequestCallback[] = [];
let now = 0;

beforeEach(() => {
  frames = [];
  now = 0;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
  vi.stubGlobal("cancelAnimationFrame", () => { frames = []; });
  mocks.taskProgress.mockResolvedValue(progress);
  mocks.taskReport.mockResolvedValue(report);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

function flushFrames() {
  const pending = frames;
  frames = [];
  for (const f of pending) f(0);
}

async function mountQa() {
  const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
  await flushPromises();
  await flushPromises();
  return w;
}

describe("ходы приёмки", () => {
  it("принять: удержание отправляет акт ровно один раз", async () => {
    const w = await mountQa();
    const btn = w.find(".p-qacts .p-qa.accept");
    expect(btn.exists(), "кнопка приёма").toBe(true);
    await btn.trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    await flushPromises();
    expect(mocks.taskAccept).toHaveBeenCalledTimes(1);
    w.unmount();
  });

  it("на доработку: уносит и слова человека, и выбранные пункты", async () => {
    const w = await mountQa();
    await w.find(".p-qanote").setValue("не чинится край списка");
    // снимаем с контроля один пункт — в причину он попасть не должен
    // в режиме «только потери» показаны два пункта (третий — 1.0, потерь нет)
    expect(w.findAll(".p-pick")).toHaveLength(2);
    await nextTick();

    const btn = w.find(".p-qacts .p-qa.back");
    // по умолчанию отмечено только то, что ниже 1.0: полный балл чинить нечего
    expect(btn.text()).toContain("На доработку · 2");
    await btn.trigger("pointerdown");
    now = HOLD_MS + 10;
    flushFrames();
    await flushPromises();

    expect(mocks.taskReturn).toHaveBeenCalledTimes(1);
    const rationale = String((mocks.taskReturn.mock.calls[0] as unknown as [string, string])[1]);
    expect(rationale).toContain("не чинится край списка");
    // причины — слова человека, потом перечень пунктов, которые он оставил
    // «на доработку» (по умолчанию все: снятие с контроля это личный выбор)
    expect(rationale.split("\n")[0]).toBe("не чинится край списка");
    expect(rationale).toContain("пункты: cl/style-consistency, cl/short-units");
    expect(rationale).not.toContain("ts/coverage-static");
    w.unmount();
  });

  it("отпускание раньше порога ничего не отправляет", async () => {
    const w = await mountQa();
    const btn = w.find(".p-qacts .p-qa.back");
    await btn.trigger("pointerdown");
    now = HOLD_MS - 1;
    await btn.trigger("pointerup");
    flushFrames();
    await flushPromises();
    expect(mocks.taskReturn).not.toHaveBeenCalled();
    w.unmount();
  });
});
