// Консоль прогона должна ЧИТАТЬСЯ: ответ роли приходит одним JSON, и он
// раскладывается по местам — счётчики чипами, находки списком с абзацами.
// Сырой поток остаётся доступен тумблером «как есть».
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import RunConsole from "../proto/RunConsole.vue";

const body = JSON.stringify({
  verdict: "APPROVED",
  scores: { "cl/meaningful-names": 1.0, "cl/short-units": 0.7, "arch/no-magic": 0.4 },
  findings: [
    { check: "cl/meaningful-names", text: "Имена читаются назначением.\nВторой строкой — пояснение." },
    { check: "arch/no-magic", text: "Магическое число 380 в панели." },
  ],
  human_paragraph: "Короткая выжимка для человека.",
});

describe("консоль прогона", () => {
  it("JSON раскладывается по местам: счётчики чипами, находки списком", () => {
    const w = mount(RunConsole, { props: { text: body } });
    expect(w.findAll(".rchips .rchip")).toHaveLength(3);
    // 1.0 зелёный, 0.7 середина, 0.4 плохо — цвет это оценка, а не украшение
    const cls = w.findAll(".rchip").map((c) => c.classes().filter((x) => ["ok", "mid", "bad"].includes(x))[0]);
    expect(cls).toEqual(["ok", "mid", "bad"]);

    const rows = w.findAll(".rclist li");
    expect(rows).toHaveLength(2);
    expect(rows[0].find(".rh").text()).toBe("cl/meaningful-names");
    // экранированный перевод строки развёрнут: абзац читается абзацем
    expect(rows[0].find(".rt").text()).toContain("Второй строкой");
    expect(rows[0].find(".rt").element.textContent).toContain("\n");

    // короткая строка — пара «ключ: значение», длинная (с переносами) — абзац
    expect(w.findAll(".rckv").map((x) => x.text())).toContain("Короткая выжимка для человека.");
  });

  it("тумблер «как есть» показывает ровно поток, без разбора", async () => {
    const w = mount(RunConsole, { props: { text: body } });
    expect(w.find(".rcraw").exists()).toBe(false);
    await w.findAll(".rcset")[1].trigger("click");
    expect(w.find(".rcraw").text()).toBe(body);
    expect(w.find(".rcbody").exists()).toBe(false);
  });

  it("не-JSON поток не притворяется структурой", () => {
    const w = mount(RunConsole, { props: { text: "пишу патч\nготово\n" } });
    expect(w.find(".rcbody").exists()).toBe(false);
    expect(w.find(".rcraw").text()).toContain("пишу патч");
  });
});
