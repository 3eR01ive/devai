// Мини-график сходимости (REQ-CV-10): точки, разрывы, линия порога и
// подписи. Проверяется РИСОВАНИЕ готовых значений — компонент ничего не
// пересчитывает, поэтому здесь нет ни одного ожидания про «правильный»
// балл: балл приходит в фикстуре таким, каким его посчитала платформа.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import ConvergenceChart from "../components/ConvergenceChart.vue";
import {
  lastRoundLabel,
  missingLabel,
  modeLabel,
  plot,
  type ChartPoint,
} from "../convergence_view";

const point = (over: Partial<ChartPoint> & { round: number }): ChartPoint => ({
  measured: true,
  score_milli: 900,
  theta_milli: 950,
  requirements_count: null,
  oq_count: null,
  modes: [],
  profile: "devai@1",
  ...over,
});

function mountChart(props: Record<string, unknown>) {
  return mount(ConvergenceChart, { props: props as never });
}

describe("convergence vocabulary", () => {
  it("maps every platform token to a human phrase and keeps unknown ones as-is", () => {
    expect(modeLabel("CONVERGING")).toBe("сходится");
    expect(modeLabel("PLATEAU")).toBe("остановилась");
    expect(modeLabel("DIVERGING")).toBe("расходится");
    expect(modeLabel("STUCK")).toBe("застряла");
    expect(modeLabel("SOMETHING_NEW")).toBe("SOMETHING_NEW");
    expect(missingLabel("report_body_absent")).toBe("тела отчёта нет");
    expect(lastRoundLabel(["CONVERGING", "STUCK"], null)).toBe("сходится + застряла");
    expect(lastRoundLabel([], "plateau_pending")).toContain("прирост ниже шага");
  });
});

describe("plot", () => {
  it("breaks the line at an unmeasured round and never joins through it", () => {
    const series = [
      point({ round: 1 }),
      point({ round: 2, measured: false, score_milli: null }),
      point({ round: 3 }),
    ];
    const p = plot(series, "quality", true);
    expect(p.segments).toEqual([[0], [2]]);
    expect(p.points[2].breaksBefore).toBe(true);
  });

  it("breaks the line at a profile segment boundary", () => {
    const series = [
      point({ round: 1, profile: "devai@12" }),
      point({ round: 2, profile: "devai@13" }),
    ];
    const withBreak = plot(series, "quality", true);
    expect(withBreak.segments).toEqual([[0], [1]]);
    const noBreak = plot(series, "quality", false);
    expect(noBreak.segments).toEqual([[0, 1]]);
  });

  it("draws a threshold step only where the stored theta exists", () => {
    const series = [
      point({ round: 1, theta_milli: 950 }),
      point({ round: 2, theta_milli: null }),
      point({ round: 3, theta_milli: 0 }),
    ];
    const p = plot(series, "quality", false);
    expect(p.threshold.length).toBe(0); // no segment spans a missing theta
  });

  it("keeps a theta of 0 as a line on zero and never substitutes 0.80", () => {
    const p = plot([point({ round: 1, theta_milli: 0 }), point({ round: 2, theta_milli: 0 })], "quality", false);
    expect(p.threshold.length).toBe(1);
    expect(p.threshold[0].y1).toBeCloseTo(p.threshold[0].y2, 5);
    expect(p.threshold[0].y1).toBeGreaterThan(70); // низ графика, не 0.80 (≈24)
  });
});

describe("ConvergenceChart", () => {
  it("says «кругов пока нет» without axes when the stage has no rounds", () => {
    const w = mountChart({ stage: "quality", series: [], gapMilli: 10, modes: [], modeNote: null, missing: [] });
    expect(w.text()).toContain("кругов пока нет");
    expect(w.find("svg").exists()).toBe(false);
    w.unmount();
  });

  it("plots one round, labels it and says the data is thin", () => {
    const w = mountChart({
      stage: "quality",
      series: [point({ round: 1, modes: ["CONVERGING"] })],
      gapMilli: 10,
      modes: ["CONVERGING"],
      modeNote: null,
      missing: [],
    });
    expect(w.find("svg").exists()).toBe(true);
    expect(w.text()).toContain("круг 1");
    expect(w.text()).toContain("сходится");
    expect(w.text()).toContain("данных мало");
    w.unmount();
  });

  it("prints every mode of the last round and its missing reasons", () => {
    const w = mountChart({
      stage: "quality",
      series: [point({ round: 1 }), point({ round: 2, modes: ["DIVERGING", "STUCK"] })],
      gapMilli: 10,
      modes: ["DIVERGING", "STUCK"],
      modeNote: null,
      missing: ["theta_absent"],
    });
    expect(w.text()).toContain("расходится + застряла");
    expect(w.text()).toContain("порог не сохранён");
    w.unmount();
  });

  it("draws the spec stage with requirements and the OQ dashed line", () => {
    const w = mountChart({
      stage: "spec_review",
      series: [
        point({ round: 1, requirements_count: 14, oq_count: 5, score_milli: null, theta_milli: null }),
        point({ round: 2, requirements_count: 22, oq_count: 2, score_milli: null, theta_milli: null }),
      ],
      gapMilli: 10,
      modes: ["DIVERGING"],
      modeNote: null,
      missing: [],
    });
    expect(w.findAll("polyline.line").length).toBe(2); // requirements + OQ
    expect(w.find("polyline.oq").exists()).toBe(true);
    expect(w.find("line.threshold").exists()).toBe(false); // no threshold on spec
    w.unmount();
  });
});