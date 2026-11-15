// Постановка задачи из списка: форма в самой вкладке, без модальных окон;
// без названия кнопка недоступна; создание уходит в платформу и открывает карточку.
import { mount } from "@vue/test-utils";
import { flushPromises } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  tasksList: vi.fn(),
  taskProgress: vi.fn(),
  taskUsage: vi.fn(),
  taskNew: vi.fn(),
  projectsList: vi.fn(),
  push: vi.fn(),
}));

vi.mock("vue-router", () => ({
  useRouter: () => ({ push: mocks.push }),
}));

vi.mock("../api", () => ({
  api: {
    tasksList: mocks.tasksList,
    taskProgress: mocks.taskProgress,
    taskUsage: mocks.taskUsage,
    taskNew: mocks.taskNew,
    projectsList: mocks.projectsList,
  },
  toUiError: (e: unknown) => e,
}));

import ProtoTasks from "../proto/ProtoTasks.vue";

const stub = { RouterLink: { template: "<a><slot /></a>" } };

describe("ProtoTasks / новая задача", () => {
  it("форма открывается кнопкой и требует название", async () => {
    mocks.tasksList.mockResolvedValue([]);
    mocks.projectsList.mockResolvedValue([]);
    const w = mount(ProtoTasks, { global: { stubs: stub } });
    await flushPromises();

    expect(w.find(".p-newform").exists()).toBe(false);
    await w.find(".p-new").trigger("click");
    expect(w.find(".p-newform").exists()).toBe(true);
    expect(w.find(".p-create").attributes("disabled")).toBeDefined();

    await w.find(".p-newform input").setValue("Новая задача");
    expect(w.find(".p-create").attributes("disabled")).toBeUndefined();
  });

  it("создание уходит в платформу и открывает карточку", async () => {
    mocks.tasksList.mockResolvedValue([]);
    mocks.projectsList.mockResolvedValue([]);
    mocks.taskNew.mockResolvedValue({ id: "01a1ffff-0000-7000-8000-000000000001" });

    const w = mount(ProtoTasks, { global: { stubs: stub } });
    await flushPromises();
    await w.find(".p-new").trigger("click");
    await w.find(".p-newform input").setValue("Нужна остановка по кругам");
    await w.find(".p-newform textarea").setValue("Постановка: останавливать и спрашивать.");
    await w.find(".p-create").trigger("click");
    await flushPromises();

    expect(mocks.taskNew).toHaveBeenCalledWith(
      "Нужна остановка по кругам",
      "Постановка: останавливать и спрашивать.",
      null,
    );
    expect(mocks.push).toHaveBeenCalledWith("/proto/task/01a1ffff-0000-7000-8000-000000000001");
  });
});
