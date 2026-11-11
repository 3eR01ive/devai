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
import { ExecutionStatusToken, LivenessToken } from "../states";
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
  taskRun: vi.fn(),
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

function running(liveness: string, line: string, id = "e1", role = "implementation-agent") {
  return {
    id,
    role: `${role}@1`,
    role_slug: role,
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
    // Токен платформы как есть: карточка сравнивает его словарём states.ts.
    status: ExecutionStatusToken.Running,
    inputs: [],
    summary: null,
    error: null,
    started_at: r.started_at,
    finished_at: null,
    liveness: r.liveness,
    liveness_line: r.liveness_line,
  }));
}

function wire(rows: ReturnType<typeof running>[], opts: { terminal?: boolean } = {}) {
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
      terminal: opts.terminal ?? false,
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
    wire([running(LivenessToken.Alive, "процесс жив")]);
    const w = await mountView();
    const text = indicator(w);
    expect(text).toContain("implementation-agent@1: процесс жив");
    // Живое исполнение не предлагает перезапуск.
    expect(w.text()).not.toContain("devai exec reap");
  });

  it("names the death and restarts it in the window with the platform's words", async () => {
    wire([running(LivenessToken.Dead, "исполнение мертво (можно перезапустить)")]);
    mocks.taskRun.mockResolvedValue({
      run_id: "r1",
      execution_id: "e2",
      outcome: "started",
      reason: null,
      state: "IMPLEMENTATION",
      line: "прогон запущен · исполнение e2",
    });
    const w = await mountView();
    expect(indicator(w)).toContain("исполнение мертво");
    // AC3: действие — кнопка окна, и она зовёт ТУ ЖЕ операцию платформы, что
    // `devai task run` (одна мутация, никакой очерёдности в окне).
    const button = w.findAll("button").find((b) => b.text() === "перезапустить");
    expect(button, "кнопка «перезапустить» не отрендерилась").toBeDefined();
    await button!.trigger("click");
    await flushPromises();
    expect(mocks.taskRun).toHaveBeenCalledWith("t1", null, false);
    // Ответ печатается словами платформы, а не пересказом токена исхода.
    expect(w.text()).toContain("прогон запущен · исполнение e2");
  });

  // R2 (corr/spec-fidelity): «одно исполнение — один вердикт». Чип смерти
  // принадлежит СВОЕМУ исполнению: живая параллельная роль не получает
  // оформление смерти только потому, что умерла соседняя строка.
  it("marks only the dead chip, not every chip of the card", async () => {
    const dead = running(LivenessToken.Dead, "исполнение мертво (можно перезапустить)", "e1");
    const alive = running(LivenessToken.Alive, "процесс жив", "e2", "requirements-analyst");
    wire([dead, alive]);
    const w = await mountView();
    const chips = w.findAll(".chip-liveness");
    expect(chips.length).toBe(2);
    const deadChip = chips.find((c) => c.text().includes("исполнение мертво"))!;
    const aliveChip = chips.find((c) => c.text().includes("процесс жив"))!;
    expect(deadChip.classes()).toContain("dead");
    expect(aliveChip.classes()).not.toContain("dead");
  });

  // R2 (ts/coverage-static): ячейка живости таблицы исполнений несёт тот же
  // факт и ту же фразу платформы — раньше её рендер не утверждался.
  it("prints the liveness fact and the platform's sentence in the executions table", async () => {
    wire([running(LivenessToken.Unknown, "живость проверить нельзя")]);
    const w = await mountView();
    const table = w.findAll(".panel").find((p) => p.text().includes("Исполнения ролей"))!;
    expect(table.exists()).toBe(true);
    const cell = table.findAll("td").find((td) => td.attributes("data-label") === "Живость")!;
    expect(cell.text()).toContain("unknown");
    expect(cell.text()).toContain("живость проверить нельзя");
  });

  it("never claims «работает» for an unverifiable owner", async () => {
    wire([running(LivenessToken.Unknown, "живость проверить нельзя")]);
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
      const rows = [running(LivenessToken.Alive, "процесс жив")];
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

  // Rework R6: терминальная задача МОЖЕТ держать RUNNING-строку (платформа
  // классифицирует этот класс как stale = terminal || dead). Опрос не должен
  // прекращаться: иначе смерть владельца навсегда осталась бы на экране как
  // «процесс жив» — старый факт выдавался бы за текущий.
  it("keeps polling a terminal task while its snapshot still holds a RUNNING row", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const alive = running(LivenessToken.Alive, "процесс жив");
      const dead = running(LivenessToken.Dead, "исполнение мертво (можно перезапустить)");
      wire([alive], { terminal: true });
      mocks.executionsList
        .mockResolvedValueOnce(execRows([alive]))
        .mockResolvedValueOnce(execRows([dead]));

      const w = await mountView();
      expect(indicator(w)).toContain("процесс жив");

      await vi.advanceTimersByTimeAsync(pollMs());
      await flushPromises();

      expect(mocks.executionsList).toHaveBeenCalledTimes(2);
      expect(indicator(w)).toContain("исполнение мертво");
    } finally {
      vi.useRealTimers();
    }
  });

  // Обратная сторона той же границы: терминальная задача без RUNNING-строк
  // новых строк не родит — дешёвое чтение на каждом тике ей не нужно.
  it("stops polling a terminal task with no running row", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      wire([], { terminal: true });
      const w = await mountView();
      expect(mocks.executionsList).toHaveBeenCalledTimes(1);

      await vi.advanceTimersByTimeAsync(pollMs());
      await flushPromises();

      expect(mocks.executionsList).toHaveBeenCalledTimes(1);
      expect(w.find(".chip-liveness").exists()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  // Rework R6: отказ запуска — слова платформы РЯДОМ с кнопкой, и кнопка
  // снова доступна (ветвь restartError не была покрыта).
  it("shows the platform's refusal beside the restart button and re-enables it", async () => {
    wire([running(LivenessToken.Dead, "исполнение мертво (можно перезапустить)")]);
    mocks.taskRun.mockRejectedValue({
      code: "already_running",
      message: "задачу держит другое исполнение",
    });
    const w = await mountView();
    const button = w.findAll("button").find((b) => b.text() === "перезапустить")!;
    await button.trigger("click");
    await flushPromises();

    expect(w.text()).toContain("already_running");
    expect(w.text()).toContain("задачу держит другое исполнение");
    const after = w.findAll("button").find((b) => b.text() === "перезапустить");
    expect(after, "кнопка остаётся на месте после отказа").toBeDefined();
    expect(after!.attributes("disabled")).toBeUndefined();
  });

  // Rework R6: пока запуск в полёте, кнопка занята и второй клик не рождает
  // второй мутации (ветвь restartBusy не была покрыта).
  it("holds the restart button busy while the launch is in flight", async () => {
    wire([running(LivenessToken.Dead, "исполнение мертво (можно перезапустить)")]);
    let finish!: (outcome: unknown) => void;
    mocks.taskRun.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const w = await mountView();
    const button = w.findAll("button").find((b) => b.text() === "перезапустить")!;
    await button.trigger("click");
    await flushPromises();

    expect(button.attributes("disabled")).toBeDefined();
    expect(button.text()).toBe("Перезапуск…");
    await button.trigger("click");
    expect(mocks.taskRun).toHaveBeenCalledTimes(1);

    finish({
      run_id: "r1",
      execution_id: "e2",
      outcome: "started",
      reason: null,
      state: "IMPLEMENTATION",
      line: "прогон запущен · исполнение e2",
    });
    await flushPromises();
    expect(w.text()).toContain("прогон запущен · исполнение e2");
  });
});
