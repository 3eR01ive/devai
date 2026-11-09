// Мини-график сходимости (REQ-CV-13): точки, разрывы, линия порога,
// две счётные линии spec и подписи. Проверяется РИСОВАНИЕ готовых
// значений — компонент ничего не пересчитывает, поэтому здесь нет ни
// одного ожидания про «правильный» балл: балл приходит в фикстуре таким,
// каким его посчитала платформа.
import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import ConvergenceChart from "../components/ConvergenceChart.vue";
import { basisLabel, missingLabel, modeLabel, plot, scoreText, stageForRow } from "../convergence_view";
import type { ConvergencePoint } from "../types";

const point = (over: Partial<ConvergencePoint> & { round: number }): ConvergencePoint => ({
  artifact_id: `a-${over.round}`,
  created_at: "2026-11-07T00:00:00Z",
  stage: "quality",
  status: "CURRENT",
  measured: true,
  comparable: true,
  missing: [],
  score_milli: 900,
  theta_milli: 950,
  low_count: 0,
  unscored_count: 0,
  invalid_count: 0,
  pending_count: 0,
  closed_count: 0,
  delta_score_milli: null,
  verdict: null,
  requirements_count: null,
  oq_count: null,
  delta_requirements: null,
  delta_oq: null,
  volume_direction: null,
  time_seconds: 60,
  profile: "devai@1",
  mode: "UNKNOWN",
  mode_basis: { rule: "unknown", reason: "insufficient_data", rounds: [], check_id: null },
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
    expect(modeLabel("UNKNOWN")).toBe("режим не определён");
    expect(modeLabel("SOMETHING_NEW")).toBe("SOMETHING_NEW");
    expect(missingLabel("quality_report")).toContain("балл");
    expect(missingLabel("stage_time")).toContain("времени");
  });

  it("prints the mode basis: STUCK names the check, UNKNOWN names the reason", () => {
    expect(
      basisLabel("STUCK", {
        rule: "stuck",
        reason: null,
        rounds: [1, 2, 3],
        check_id: "qa/opens",
      }),
    ).toBe("застряла — повтор проверки qa/opens в кругах 1, 2, 3");
    expect(
      basisLabel("UNKNOWN", {
        rule: "unknown",
        reason: "no_predicate",
        rounds: [],
        check_id: null,
      }),
    ).toContain("ни одно правило");
    expect(
      basisLabel("CONVERGING", {
        rule: "converging",
        reason: null,
        rounds: [1, 2],
        check_id: null,
      }),
    ).toContain("правило converging");
  });

  it("prints a ready milli value as three decimals without arithmetic", () => {
    expect(scoreText(994)).toBe("0.994");
    expect(scoreText(null)).toBe("—");
  });
});

describe("plot", () => {
  it("breaks the quality line at an unmeasured round and never joins through it", () => {
    const series = [
      point({ round: 1 }),
      point({ round: 2, measured: false, score_milli: null }),
      point({ round: 3 }),
    ];
    expect(plot(series, "quality", true).scoreSegments).toEqual([[0], [2]]);
  });

  it("breaks the line at a profile segment boundary", () => {
    const series = [
      point({ round: 1, profile: "devai@12" }),
      point({ round: 2, profile: "devai@13" }),
    ];
    expect(plot(series, "quality", true).scoreSegments).toEqual([[0], [1]]);
    expect(plot(series, "quality", false).scoreSegments).toEqual([[0, 1]]);
  });

  it("draws a threshold step only where the stored theta exists", () => {
    const series = [
      point({ round: 1, theta_milli: 950 }),
      point({ round: 2, theta_milli: null }),
      point({ round: 3, theta_milli: 0 }),
    ];
    expect(plot(series, "quality", false).threshold.length).toBe(0);
  });

  it("keeps a theta of 0 as a line on zero and never substitutes 0.80", () => {
    const p = plot(
      [point({ round: 1, theta_milli: 0 }), point({ round: 2, theta_milli: 0 })],
      "quality",
      false,
    );
    expect(p.threshold.length).toBe(1);
    expect(p.threshold[0].y1).toBeGreaterThan(70); // низ графика, не 0.80
  });

  it("draws BOTH spec counters on one scale and breaks only the null one", () => {
    const series = [
      point({ round: 1, requirements_count: 14, oq_count: 5, score_milli: null }),
      point({ round: 2, requirements_count: 14, oq_count: null, score_milli: null }),
      point({ round: 3, requirements_count: 14, oq_count: 0, score_milli: null }),
    ];
    const p = plot(series, "spec_review", true);
    expect(p.reqSegments).toEqual([[0, 1, 2]]);
    // null (ключа нет) рвёт линию, явный 0 рисуется нулём
    expect(p.oqSegments).toEqual([[0], [2]]);
    expect(p.top).toBe(14);
  });
});

describe("stageForRow", () => {
  const view = {
    gap_milli: 10,
    stages: [
      {
        stage: "spec_review",
        series: [point({ round: 1 })],
        counters: {
          rounds_total: 1,
          rounds_unmeasured: 0,
          rounds_without_time: 0,
          spec_versions_without_review: 0,
        },
        stage_total_seconds: null,
        stage_current_seconds: null,
        missing: [],
      },
      {
        stage: "quality",
        series: [point({ round: 1 }), point({ round: 2 })],
        counters: {
          rounds_total: 2,
          rounds_unmeasured: 0,
          rounds_without_time: 0,
          spec_versions_without_review: 0,
        },
        stage_total_seconds: null,
        stage_current_seconds: null,
        missing: [],
      },
    ],
  };

  it("picks the gate's own stage and the LAST stage for a holding row", () => {
    expect(stageForRow(view, "APPROVAL").stage).toBe("spec_review");
    expect(stageForRow(view, "QA").stage).toBe("quality");
    // состояние удержания: последний элемент stages[], независимо от формы гейта
    expect(stageForRow(view, "WAITING_FOR_INPUT").stage).toBe("quality");
    expect(stageForRow(view, "WAITING_FOR_INPUT").series.length).toBe(2);
  });
});

describe("ConvergenceChart", () => {
  it("says «кругов пока нет» without axes when the stage has no rounds", () => {
    const w = mountChart({ stage: "quality", series: [], gapMilli: 10 });
    expect(w.text()).toContain("кругов пока нет");
    expect(w.find("svg").exists()).toBe(false);
    w.unmount();
  });

  it("plots one round, labels it and names the last mode via its basis", () => {
    const w = mountChart({
      stage: "quality",
      series: [
        point({
          round: 1,
          mode: "CONVERGING",
          mode_basis: { rule: "converging", reason: null, rounds: [1, 2], check_id: null },
        }),
      ],
      gapMilli: 10,
    });
    expect(w.find("svg").exists()).toBe(true);
    expect(w.text()).toContain("круг 1");
    expect(w.text()).toContain("сходится");
    expect(w.text()).toContain("S=0.900");
    expect(w.text()).toContain("данных мало");
    w.unmount();
  });

  it("prints the last point's ready value and the gaps honestly", () => {
    const w = mountChart({
      stage: "quality",
      series: [
        point({ round: 1 }),
        point({
          round: 2,
          score_milli: 994,
          low_count: 1,
          theta_milli: null,
          mode: "DIVERGING",
          mode_basis: { rule: "diverging", reason: null, rounds: [1, 2], check_id: null },
          missing: [{ field: "theta_milli", token: "quality_theta" }],
        }),
      ],
      gapMilli: 10,
    });
    expect(w.text()).toContain("расходится");
    expect(w.text()).toContain("S=0.994");
    expect(w.text()).toContain("пунктов ниже 1.0: 1");
    expect(w.text()).toContain("порог неизвестен");
    expect(w.text()).toContain("theta_milli: порог не сохранён");
    w.unmount();
  });

  it("draws the spec stage with requirements AND open questions, no threshold", () => {
    const w = mountChart({
      stage: "spec_review",
      series: [
        point({
          round: 1,
          requirements_count: 14,
          oq_count: 5,
          score_milli: null,
          theta_milli: null,
          stage: "spec_review",
        }),
        point({
          round: 2,
          requirements_count: 22,
          oq_count: 2,
          score_milli: null,
          theta_milli: null,
          stage: "spec_review",
          mode: "DIVERGING",
          mode_basis: { rule: "diverging", reason: null, rounds: [1, 2], check_id: null },
        }),
      ],
      gapMilli: 10,
    });
    expect(w.findAll("polyline.line").length).toBe(2); // требования + OQ
    expect(w.find("polyline.oq").exists()).toBe(true);
    expect(w.find("polyline.req").exists()).toBe(true);
    expect(w.find("line.threshold").exists()).toBe(false);
    expect(w.text()).toContain("требований 22");
    expect(w.text()).toContain("открытых вопросов 2");
    w.unmount();
  });

  it("shows UNKNOWN as a neutral badge with its reason", () => {
    const w = mountChart({ stage: "quality", series: [point({ round: 1 })], gapMilli: 10 });
    const badge = w.find(".conv-last .mode");
    expect(badge.classes()).toContain("unknown");
    expect(badge.text()).toContain("режим не определён");
    expect(badge.text()).toContain("кругов мало");
    w.unmount();
  });

  it("prints the stage counters: unmeasured rounds, waiting signatures, defects", () => {
    const w = mountChart({
      stage: "quality",
      series: [
        point({
          round: 1,
          measured: false,
          score_milli: null,
          low_count: null,
          unscored_count: null,
          invalid_count: null,
          pending_count: null,
        }),
        point({ round: 2, invalid_count: 2, pending_count: 1 }),
      ],
      gapMilli: 10,
    });
    const counters = w.find(".conv-counters").text();
    expect(counters).toContain("не измерено 1");
    expect(counters).toContain("ждут 1");
    expect(counters).toContain("дефект 2");
    w.unmount();
  });
});
