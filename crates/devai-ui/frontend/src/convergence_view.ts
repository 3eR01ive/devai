// Словарь и геометрия мини-графика сходимости (REQ-CV-13).
//
// Два правила этого файла:
//  1. Значения НЕ пересчитываются: балл, порог, объём, режим и причина
//     приходят посчитанными платформой (`task.progress` / `inbox.snapshot`).
//     Здесь только фиксированный человеческий словарь для машинных токенов
//     и раскладка точек по пикселям.
//  2. Линия рвётся там, где данные рвутся: неизмеренный круг, null-счётчик,
//     граница профиля — разрыв, а не соединение через пустоту и не ноль.

import type { ConvergencePoint, MissingEntry, ModeBasis, ConvergenceView, StageConvergence } from "./types";
import { TaskStateToken } from "./states";

/** Один круг в том виде, в каком его рисует компонент. */
export type ChartPoint = ConvergencePoint;

/** Режимы (REQ-CV-09) — закрытый словарь, включая UNKNOWN. */
export const MODE_LABELS: Record<string, string> = {
  CONVERGING: "сходится",
  PLATEAU: "остановилась",
  DIVERGING: "расходится",
  STUCK: "застряла",
  UNKNOWN: "режим не определён",
};

/** Причины UNKNOWN (REQ-CV-09). */
export const REASON_LABELS: Record<string, string> = {
  insufficient_data: "кругов мало или предыдущий круг не измерен",
  no_predicate: "ни одно правило не сработало",
};

/** Направление объёма spec-точки (REQ-CV-06). */
export const VOLUME_LABELS: Record<string, string> = {
  growing: "объём растёт",
  shrinking: "объём сокращается",
  flat: "объём не меняется",
};

/** Токены пробелов данных (REQ-CV-05) — закрытый словарь + `read_error`
 *  (QA R2: сбой чтения стора назван отдельно от битого тела). */
export const MISSING_LABELS: Record<string, string> = {
  quality_report: "тело отчёта не даёт балл и проверки",
  quality_theta: "порог не сохранён",
  review_verdict: "вердикт не прочитан",
  spec_binding: "ревью не привязано к версии спеки",
  spec_body: "тело спеки не прочитано",
  spec_counts: "в спеке нет нужного массива",
  counts_unavailable: "счётчика нет у одного из двух кругов",
  time: "время круга неизвестно",
  stage_time: "ни у одного круга нет времени",
  profile: "профиль оценки не прочитан",
  current_unmeasured: "круг не измерен",
  predecessor_unmeasured: "предыдущий круг не измерен",
  segment_boundary: "сменился профиль оценки",
  read_error: "сбой чтения из стора",
};

export function modeLabel(token: string): string {
  return MODE_LABELS[token] ?? token;
}

export function reasonLabel(token: string | null): string {
  if (!token) return "";
  return REASON_LABELS[token] ?? token;
}

export function missingLabel(token: string): string {
  return MISSING_LABELS[token] ?? token;
}

export function volumeLabel(token: string | null): string {
  if (!token) return "";
  return VOLUME_LABELS[token] ?? token;
}

/** Строка бейджа: режим + основание (правило / причина / повтор проверки). */
export function basisLabel(mode: string, basis: ModeBasis | null): string {
  const head = modeLabel(mode);
  if (!basis) return head;
  if (mode === "STUCK") {
    const id = basis.check_id ? `проверки ${basis.check_id}` : "проверки";
    return `${head} — повтор ${id} в кругах ${basis.rounds.join(", ")}`;
  }
  if (mode === "UNKNOWN") {
    return `${head} — ${reasonLabel(basis.reason)}`;
  }
  return `${head} — правило ${basis.rule}`;
}

/** Готовый балл в виде 0.994 — без пересчёта из милли. */
export function scoreText(milli: number | null): string {
  if (milli === null) return "—";
  return (milli / 1000).toFixed(3);
}

/** Точка сетки: пиксельные координаты одного круга. */
export interface Plotted {
  round: number;
  x: number;
  /** Ордината главной метрики (quality — балл, spec — требования). */
  y: number;
  measured: boolean;
  profile: string | null;
  mode: string;
}

export interface Plot {
  points: Plotted[];
  /** Непрерывные участки линий; индексы точек между разрывами. */
  scoreSegments: number[][];
  reqSegments: number[][];
  oqSegments: number[][];
  /** Порог ступенью по сохранённому theta каждого круга (только quality). */
  threshold: { x1: number; y1: number; x2: number; y2: number }[];
  width: number;
  height: number;
  /** Верх шкалы: 1 для quality, максимум счётчиков для spec. */
  top: number;
}

const W = 320;
const H = 96;
const PAD_X = 10;
const PAD_TOP = 10;
const PAD_BOTTOM = 18;

function xOf(round: number, count: number): number {
  if (count <= 1) return W / 2;
  return PAD_X + ((round - 1) / (count - 1)) * (W - 2 * PAD_X);
}

function yOf(value: number, top: number): number {
  const safeTop = Math.max(top, 1e-9);
  return PAD_TOP + (1 - Math.min(value, safeTop) / safeTop) * (H - PAD_TOP - PAD_BOTTOM);
}

/** Непрерывные участки по значению метрики; null/неизмеренный — разрыв. */
function segments(
  series: ChartPoint[],
  value: (p: ChartPoint) => number | null,
  breakOnProfile: boolean,
): number[][] {
  const out: number[][] = [];
  let current: number[] = [];
  series.forEach((p, i) => {
    const v = p.measured ? value(p) : null;
    const prev = i > 0 ? series[i - 1] : null;
    const profileBreak =
      breakOnProfile && prev !== null && (prev.profile !== p.profile || p.profile === null);
    if (v === null) {
      if (current.length) out.push(current);
      current = [];
      return;
    }
    if (profileBreak && current.length) {
      out.push(current);
      current = [];
    }
    current.push(i);
  });
  if (current.length) out.push(current);
  return out;
}

/**
 * Раскладка серии по осям. quality — балл score_milli/1000 и порог;
 * spec_review — ДВЕ счётные линии (требования и открытые вопросы) на
 * общей оси «штуки».
 *
 * Разрывы: неизмеренный круг и null-счётчик рвут линию всегда. Граница
 * профиля рвёт ТОЛЬКО quality: профиль — свойство quality-точки
 * (REQ-CV-10), у spec его нет и быть не может, поэтому `profile === null`
 * на каждом spec-круге НЕ является разрывом (иначе две счётные линии
 * вырождаются в точки — QA R3).
 */
export function plot(series: ChartPoint[], stage: string, breakOnProfile: boolean): Plot {
  const isQuality = stage === "quality";
  const top = isQuality
    ? 1
    : Math.max(
        1,
        ...series.map((p) => Math.max(p.requirements_count ?? 0, p.oq_count ?? 0)),
      );

  const points: Plotted[] = series.map((p) => {
    const value = isQuality ? (p.score_milli ?? 0) / 1000 : (p.requirements_count ?? 0);
    return {
      round: p.round,
      x: xOf(p.round, series.length),
      y: yOf(value, top),
      measured: p.measured,
      profile: p.profile,
      mode: p.mode,
    };
  });

  const scoreSegments = isQuality
    ? segments(series, (p) => p.score_milli, breakOnProfile)
    : [];
  const reqSegments = isQuality
    ? []
    : segments(series, (p) => p.requirements_count, false);
  const oqSegments = isQuality
    ? []
    : segments(series, (p) => p.oq_count, false);

  const threshold: Plot["threshold"] = [];
  if (isQuality) {
    for (let i = 0; i < series.length - 1; i += 1) {
      const a = series[i];
      const b = series[i + 1];
      if (!a.measured || !b.measured) continue;
      if (a.theta_milli === null || b.theta_milli === null) continue;
      threshold.push({
        x1: points[i].x,
        y1: yOf(a.theta_milli / 1000, 1),
        x2: points[i + 1].x,
        y2: yOf(b.theta_milli / 1000, 1),
      });
    }
  }

  return {
    points,
    scoreSegments,
    reqSegments,
    oqSegments,
    threshold,
    width: W,
    height: H,
    top,
  };
}

/** Пробелы последнего круга в человеческих фразах (по одному на поле). */
export function gapsText(missing: MissingEntry[]): string {
  return missing.map((m) => `${m.field}: ${missingLabel(m.token)}`).join(" · ");
}

/**
 * Стадия, которую рисует строка «Ожидания» (REQ-CV-13): согласование →
 * spec_review, проверка качества → quality, иное состояние → последний
 * элемент stages[]. Выбор зависит только от stored-состояния, никогда от
 * того, отрисована ли форма гейта. Пустой stages[] даёт честную пустую
 * серию своей стадии.
 */
export function stageForRow(
  view: ConvergenceView,
  state: string,
): { stage: string; series: ConvergencePoint[] } {
  const gate =
    state === TaskStateToken.Approval
      ? "spec_review"
      : state === TaskStateToken.Qa
        ? "quality"
        : null;
  if (gate) {
    return {
      stage: gate,
      series: view.stages.find((s: StageConvergence) => s.stage === gate)?.series ?? [],
    };
  }
  const last = view.stages[view.stages.length - 1];
  return last ? { stage: last.stage, series: last.series } : { stage: "quality", series: [] };
}
