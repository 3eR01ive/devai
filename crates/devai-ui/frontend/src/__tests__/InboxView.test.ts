// Компонентные тесты секции ожидания: каждое текстовое
// ответвление шаблона InboxView.vue прогоняется на данных снапшота —
// якорная строка APPROVAL, «не current»-метка, подстановка бизнес-описания,
// честные заглушки WAITING/QA, раздельные группы «остановлено оператором»
// и справочных заметок. Сеть/IPC не участвует: api замокан.
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import InboxView from "../views/InboxView.vue";
import { resetWaitStoreForTests } from "../inbox";
import {
  artifact,
  note,
  question,
  report,
  row,
  snapshot,
  spec,
  task,
} from "./fixtures";

const mocks = vi.hoisted(() => ({
  inboxView: vi.fn(),
  sessionInfo: vi.fn(),
}));

vi.mock("../api", () => ({
  api: { inboxView: mocks.inboxView, sessionInfo: mocks.sessionInfo },
  toUiError: (e: unknown) => e,
}));

const stubs = { RouterLink: { template: "<a><slot /></a>" } };

async function mountWith(snap: unknown) {
  mocks.inboxView.mockResolvedValue(snap);
  const wrapper = mount(InboxView, { global: { stubs } });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  resetWaitStoreForTests();
  mocks.inboxView.mockReset();
});

afterEach(() => {
  resetWaitStoreForTests();
});

describe("InboxView branches", () => {
  it("empty section says the platform waits nowhere", async () => {
    const w = await mountWith(snapshot());
    expect(w.text()).toContain("Платформа нигде не ждёт оператора");
    expect(w.text()).not.toContain("От вас ждут");
    w.unmount();
  });

  it("APPROVAL presents the human paragraph, the spec's OWN open questions and the not-current mark", async () => {
    const t = task("aaaaaaaa-1111", "APPROVAL", "спека на подпись");
    const w = await mountWith(
      snapshot({
        waiting: [row(t, { spec: spec({ current: false }) })],
        badge: 1,
      }),
    );
    expect(w.text()).not.toContain("человеческий абзац спеки");
    await w.find(".panel-toggle").trigger("click");
    const text = w.text();
    expect(text).toContain("not current");
    expect(text).toContain("утвердить нельзя");
    expect(text).toContain("человеческий абзац спеки");
    expect(text).toContain("Открытые вопросы спецификации (2)");
    expect(text).toContain("OQ-1: первый вопрос документа");
    expect(text).toContain("вопросы документа, не к вам");
    expect(text).toContain("От вас ждут: demand for APPROVAL");
    expect(text).toContain("devai task ... aaaaaaaa-1111");
    w.unmount();
  });

  it("APPROVAL without a spec artifact says there is nothing to approve", async () => {
    const t = task("bbbb", "APPROVAL");
    const w = await mountWith(snapshot({ waiting: [row(t)], badge: 1 }));
    await w.find(".panel-toggle").trigger("click");
    expect(w.text()).toContain("ни одного спецификационного артефакта");
    w.unmount();
  });

  it("paragraph fallback from the business description is labelled honestly", async () => {
    const t = task("cccc", "APPROVAL");
    const w = await mountWith(
      snapshot({
        waiting: [
          row(t, {
            spec: spec({
              human_paragraph: "описание из задачи",
              paragraph_source: "business_description",
              unavailable_note: "абзац не опубликован",
            }),
          }),
        ],
        badge: 1,
      }),
    );
    await w.find(".panel-toggle").trigger("click");
    expect(w.text()).toContain("абзац: абзац не опубликован");
    expect(w.text()).toContain("описание из задачи");
    expect(w.text()).toContain("абзац подставлен из бизнес-описания");
    w.unmount();
  });

  it("WAITING shows the blocking platform question, its options and the exact CLI hint", async () => {
    const t = task("dddd", "WAITING_FOR_INPUT");
    const w = await mountWith(
      snapshot({
        waiting: [row(t, { open_questions: [question("q-1")] })],
        badge: 1,
      }),
    );
    await w.find(".panel-toggle").trigger("click");
    const text = w.text();
    expect(text).toContain("Вопросы платформы к оператору (1)");
    expect(text).toContain("блокирует");
    expect(text).toContain("вопрос q-1");
    expect(text).toContain("контекст вопроса");
    expect(text).toContain("да");
    w.unmount();
  });

  it("WAITING without an OPEN registry question shows the empty-rework honest note", async () => {
    const t = task("eeee", "WAITING_FOR_INPUT");
    const w = await mountWith(snapshot({ waiting: [row(t)], badge: 1 }));
    await w.find(".panel-toggle").trigger("click");
    expect(w.text()).toContain("гейт пустого реворка");
    w.unmount();
  });

  it("QA presents report text, a machine snippet when text is missing, and a report note", async () => {
    const withText = row(task("ffff", "QA"), { report: report() });
    const noText = row(task("gggg", "QA"), {
      report: report({ report_text: null, report: { score: 0.4 } }),
    });
    const withNote = row(task("hhhh", "QA"), {
      report_note: "quality-отчёта нет: QA не прогонялся",
    });
    const w = await mountWith(
      snapshot({ waiting: [withText, noText, withNote], badge: 3 }),
    );
    for (const toggle of await w.findAll(".panel-toggle")) {
      await toggle.trigger("click");
    }
    const text = w.text();
    expect(text).toContain("человеческий отчёт качества");
    expect(text).toContain("отчёт без человеческой части");
    expect(text).toContain('"score":0.4');
    expect(text).toContain("quality-отчёта нет: QA не прогонялся");
    w.unmount();
  });

  it("operator-stopped tasks and unanswered non-blocking notes stay in their own groups", async () => {
    const stopped = task("iiii", "PAUSED", "остановлена мной");
    const noted = task("jjjj", "IMPLEMENTATION", "активная задача");
    const w = await mountWith(
      snapshot({
        human_stopped: [row(stopped)],
        notes: [note(noted, question("q-9", { blocking: false }))],
        badge: 2,
      }),
    );
    const text = w.text();
    expect(text).toContain("Остановленные оператором");
    expect(text).toContain("остановлена мной");
    expect(text).toContain("Справочно: безответные неблокирующие вопросы");
    expect(text).toContain("не блокирует");
    expect(text).toContain("вопрос q-9");
    expect(text).not.toContain("Платформа нигде не ждёт оператора");
    w.unmount();
  });

  it("a failed platform read keeps the section honest and shows the error", async () => {
    mocks.inboxView.mockRejectedValue({ code: "store", message: "стор недоступен" });
    const w = mount(InboxView, { global: { stubs } });
    await flushPromises();
    expect(w.text()).toContain("стор недоступен");
    w.unmount();
  });

  it("a snapshot without rows but with a note is NOT reported as fully empty", async () => {
    const w = await mountWith(
      snapshot({ notes: [note(task("kkkk", "IMPLEMENTATION"), question("q-10", { blocking: false }))], badge: 1 }),
    );
    expect(w.text()).not.toContain("Платформа нигде не ждёт оператора");
    w.unmount();
  });
});
