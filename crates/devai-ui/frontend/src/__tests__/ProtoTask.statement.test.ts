// Шаг постановки: по умолчанию ЧТЕНИЕ, правка — по кнопке, принятие редакции
// уходит в платформу. Редактор открывается на весь текст, а не в узкую щель.
import { mount } from "@vue/test-utils";
import { flushPromises } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  taskProgress: vi.fn(),
  taskReport: vi.fn(async () => null),
  taskUsage: vi.fn(async () => null),
  evaluationRules: vi.fn(async () => null),
  taskSignature: vi.fn(async () => null),
  artifactsList: vi.fn(async () => []),
  questionsList: vi.fn(async () => []),
  taskEvents: vi.fn(async () => []),
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

vi.mock("../api", () => ({
  api: mocks,
  toUiError: (e: unknown) => e,
}));

import ProtoTask from "../proto/ProtoTask.vue";

const task = {
  id: "01a1b085-0000-7000-8000-000000000001", project_id: null,
  title: "Контроль токенов", business_description: "Постановка как была: бюджет и стоп.",
  human_name: null, state: "APPROVAL", return_state: null, requirements_seq: 1,
  lock_version: 2, created_at: "2026-11-13T10:00:00Z", updated_at: "2026-11-13T10:00:00Z",
  display: "Контроль токенов",
};

const stages = [
  ["STATEMENT", "постановка", "done"], ["ANALYSIS", "анализ", "done"], ["REVIEW", "ревью", "done"],
  ["APPROVAL", "подпись", "current"], ["IMPLEMENTATION", "код", "todo"], ["QA", "приёмка", "todo"],
].map(([state, label, status]) => ({ state, label, status, gate: state === "APPROVAL" }));

const progress = {
  task, snapshot_at: "", pipeline: { stages },
  graph: { stages, nodes: [], edges: [], position: {}, rework: {}, running: [], waits: [], blocking_questions: 0, notes: [] },
  allowed_operations: [], running_executions: [], convergence: null, attempt_limit: null, agent_failure: null,
};

async function mountCard() {
  mocks.taskProgress.mockResolvedValue(progress);
  const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
  await flushPromises();
  await flushPromises();
  return w;
}

describe("ProtoTask / запуск черновика", () => {
  it("у черновика есть «Начать работу», и он запускает постановщика", async () => {
    mocks.taskProgress.mockResolvedValue({ ...progress, task: { ...task, state: "DRAFT" } });
    const w = mount(ProtoTask, { global: { stubs: { RouterLink: { template: "<a><slot /></a>" } } } });
    await flushPromises();
    await flushPromises();

    const btn = w.find(".p-start");
    expect(btn.exists()).toBe(true);
    await btn.trigger("click");
    await flushPromises();
    expect(mocks.taskStart).toHaveBeenCalledWith(task.id);
    expect(mocks.taskRun).toHaveBeenCalledWith(task.id, null, false);
  });
});

describe("ProtoTask / постановка", () => {
  it("по умолчанию показывает текст и кнопку «Править постановку»", async () => {
    const w = await mountCard();
    const step = w.findAll(".p-vstep").find((s) => s.text().includes("постановка"))!;
    await step.find(".p-vhead").trigger("click");
    expect(w.text()).toContain("Постановка как была");
    expect(w.find(".p-briefedit").exists()).toBe(false);
    expect(w.text()).toContain("Править постановку");
  });

  it("правка открывается кнопкой и закрывается «Отменой»", async () => {
    const w = await mountCard();
    const step = w.findAll(".p-vstep").find((s) => s.text().includes("постановка"))!;
    await step.find(".p-vhead").trigger("click");
    await w.findAll("button").find((b) => b.text() === "Править постановку")!.trigger("click");
    expect(w.find(".p-briefedit").exists()).toBe(true);
    expect((w.find(".p-brieftitle").element as HTMLInputElement).value).toBe("Контроль токенов");
    await w.findAll("button").find((b) => b.text() === "Отмена")!.trigger("click");
    expect(w.find(".p-briefedit").exists()).toBe(false);
  });

  it("принятие редакции уходит в платформу с новым текстом", async () => {
    const w = await mountCard();
    const step = w.findAll(".p-vstep").find((s) => s.text().includes("постановка"))!;
    await step.find(".p-vhead").trigger("click");
    await w.findAll("button").find((b) => b.text() === "Править постановку")!.trigger("click");
    await w.find(".p-brieftitle").setValue("Контроль токенов v2");
    await w.find(".p-briefedit").setValue("Новая постановка: бюджет и стоп.");
    await w.findAll("button").find((b) => b.text() === "Принять новую редакцию")!.trigger("click");
    await flushPromises();
    expect(mocks.taskRestate).toHaveBeenCalledWith(
      "01a1b085-0000-7000-8000-000000000001",
      "Контроль токенов v2",
      "Новая постановка: бюджет и стоп.",
      expect.stringContaining("новая редакция"),
    );
  });
});
