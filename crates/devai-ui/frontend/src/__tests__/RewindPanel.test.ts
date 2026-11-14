// «Ошиблись на шаге»: оба акта требуют причины, текущая стадия недоступна,
// правка постановки уходит вместе с причиной. Никаких подтверждающих окон.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import RewindPanel from "../proto/RewindPanel.vue";

const props = {
  state: "ANALYSIS",
  title: "Задача",
  description: "Как было поставлено",
};

describe("RewindPanel", () => {
  it("без причины перепройти нельзя", async () => {
    const w = mount(RewindPanel, { props });
    const go = w.findAll(".go")[0];
    expect(go.attributes("disabled")).toBeDefined();
    await w.findAll("input")[0].setValue("уперлись в постановку");
    expect(w.findAll(".go")[0].attributes("disabled")).toBeUndefined();
  });

  it("текущая стадия помечена и недоступна как цель", () => {
    const w = mount(RewindPanel, { props });
    const chips = w.findAll(".chip");
    const here = chips.find((c) => c.text() === "анализ")!;
    expect(here.classes()).toContain("here");
    expect(here.attributes("disabled")).toBeDefined();
  });

  it("перепройти: цель и причина уходят одним актом", async () => {
    const w = mount(RewindPanel, { props });
    await w.findAll("input")[0].setValue("постановка была кривая");
    const chips = w.findAll(".chip");
    await chips.find((c) => c.text() === "постановка")!.trigger("click");
    await w.findAll(".go")[0].trigger("click");
    expect(w.emitted("rewind")![0]).toEqual(["STATEMENT", "постановка была кривая"]);
  });

  it("переписать постановку: нужна причина и реальная правка", async () => {
    const w = mount(RewindPanel, { props });
    const [reasonInput, titleInput] = w.findAll("input");
    const restateBtn = () => w.findAll(".go")[1];
    expect(restateBtn().attributes("disabled")).toBeDefined();

    await reasonInput.setValue("ошиблись в постановке");
    expect(restateBtn().attributes("disabled")).toBeDefined(); // правки ещё нет

    await titleInput.setValue("Новое имя задачи");
    expect(restateBtn().attributes("disabled")).toBeUndefined();
    await restateBtn().trigger("click");
    expect(w.emitted("restate")![0]).toEqual(["Новое имя задачи", "Как было поставлено", "ошиблись в постановке"]);
  });
});
