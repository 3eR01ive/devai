// Словарь и геометрия мини-графика сходимости (REQ-CV-10).
//
// Два правила этого файла:
//  1. Значения НЕ пересчитываются: балл, режим, дельта и причина приходят
//     посчитанными платформой (`task.convergence` / факт `inbox.snapshot`).
//     Здесь только фиксированный человеческий словарь для машинных токенов
//     (REQ-CV-13: «человеческие фразы — фиксированный словарь UI») и
//     раскладка точек по пикселям.
//  2. Линия рвётся там, где данные рвутся: неизмеренный круг и граница
//     сегмента профиля — разрыв, а не соединение через пустоту. Никакого
//     сглаживания и никакой подстановки предыдущего значения.

/** Один круг в том виде, в каком его рисует компонент. */
export interface ChartPoint {
  round: number;
  measured: boolean;
  score_milli: number | null;
  theta_milli: number | null;
  requirements_count: number | null;
  oq_count: number | null;
  modes: string[];
  profile: string | null;
}

/** Токены режимов (REQ-CV-05) — закрытый словарь. */
export const MODE_LABELS: Record<string, string> = {
  CONVERGING: "сходится",
  PLATEAU: "остановилась",
  DIVERGING: "расходится",
  STUCK: "застряла",
};

/** Токены причины «режима нет» (REQ-CV-05) — закрытый словарь. */
export const NOTE_LABELS: Record<string, string> = {
  insufficient_data: "круг не измерен — данных мало",
  no_predecessor: "первый круг — сравнивать не с чем",
  profile_changed: "сменился профиль оценки — серия начинается заново",
  profile_unknown: "профиль оценки не прочитан — сравнивать не с чем",
  nonadjacent_predecessor: "предыдущий круг пропущен — сравнение не с соседним",
  plateau_pending: "прирост ниже шага — нужен ещё круг",
  progress_without_closure: "балл вырос, но ни один пункт не закрыт",
  shrinking_without_closed_items: "объём сократился, но открытые вопросы не закрыты",
  growth_with_approved: "объём вырос, но спека утверждена",
};

/** Токены пробелов данных (REQ-CV-13) — закрытый словарь. */
export const MISSING_LABELS: Record<string, string> = {
  report_body_absent: "тела отчёта нет",
  report_body_unreadable: "тело отчёта не читается",
  spec_body_absent: "тела спеки нет",
  spec_body_unreadable: "тело спеки не читается",
  score_absent: "в отчёте нет балла",
  checks_absent: "в отчёте нет списка проверок",
  verdict_absent: "в отчёте нет вердикта",
  spec_unbound: "ревью не привязано к версии спеки",
  requirements_absent: "в спеке нет списка требований",
  theta_absent: "порог не сохранён — линии порога нет",
  time_absent: "время круга неизвестно",
};

export function modeLabel(token: string): string {
  return MODE_LABELS[token] ?? token;
}

export function noteLabel(token: string | null): string {
  if (!token) return "";
  return NOTE_LABELS[token] ?? token;
}

export function missingLabel(token: string): string {
  return MISSING_LABELS[token] ?? token;
}

/** Подпись последнего круга: все режимы по порядку платформы + причина. */
export function lastRoundLabel(modes: string[], modeNote: string | null): string {
  if (modes.length) return modes.map(modeLabel).join(" + ");
  return noteLabel(modeNote);
}

/** Точка сетки: пиксельные координаты одного круга. */
export interface Plotted {
  round: number;
  x: number;
  y: number;
  measured: boolean;
  profile: string | null;
  modes: string[];
  /** Разрыв линии ПЕРЕД этой точкой (пропуск или смена сегмента). */
  breaksBefore: boolean;
}

export interface Plot {
  points: Plotted[];
  /** Непрерывные участки линии: индексы точек, между которыми линия идёт. */
  segments: number[][];
  /** Линия порога ступенью (только quality и только там, где theta есть). */
  threshold: { x1: number; y1: number; x2: number; y2: number }[];
  width: number;
  height: number;
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

/**
 * Раскладка серии по осям. `stage` решает метрику: quality — балл
 * (score_milli/1000), spec_review — требования (сплошная) и открытые
 * вопросы (пунктир). Разрывы: неизмеренный круг; для графика задачи ещё и
 * смена profile-сегмента (`breakOnProfile`).
 */
export function plot(series: ChartPoint[], stage: string, breakOnProfile: boolean): Plot {
  const count = series.length;
  const values: number[] = [];
  for (const p of series) {
    if (stage === "quality" && p.measured && p.score_milli !== null) {
      values.push(p.score_milli / 1000);
    }
  }
  let top = 1;
  if (stage !== "quality") {
    for (const p of series) {
      values.push(p.requirements_count ?? 0, p.oq_count ?? 0);
    }
    top = Math.max(1, ...values);
  }

  const points: Plotted[] = series.map((p, i) => {
    const value =
      stage === "quality" ? (p.score_milli ?? 0) / 1000 : (p.requirements_count ?? 0);
    const y = PAD_TOP + (1 - Math.min(value, top) / top) * (H - PAD_TOP - PAD_BOTTOM);
    const prev = i > 0 ? series[i - 1] : null;
    const profileBreak =
      breakOnProfile && prev !== null && (prev.profile !== p.profile || p.profile === null);
    return {
      round: p.round,
      x: xOf(p.round, count),
      y,
      measured: p.measured,
      profile: p.profile,
      modes: p.modes,
      breaksBefore: i > 0 && (!p.measured || !prev?.measured || profileBreak),
    };
  });

  const segments: number[][] = [];
  let current: number[] = [];
  points.forEach((p, i) => {
    if (i === 0) {
      if (p.measured) current = [i];
      return;
    }
    if (p.breaksBefore) {
      if (current.length) segments.push(current);
      current = p.measured ? [i] : [];
    } else if (p.measured) {
      current.push(i);
    }
  });
  if (current.length) segments.push(current);

  const threshold: Plot["threshold"] = [];
  if (stage === "quality") {
    for (let i = 0; i < points.length - 1; i += 1) {
      const a = series[i];
      const b = series[i + 1];
      if (a.theta_milli === null || b.theta_milli === null) continue;
      if (!a.measured || !b.measured) continue;
      threshold.push({
        x1: points[i].x,
        y1: PAD_TOP + (1 - a.theta_milli / 1000) * (H - PAD_TOP - PAD_BOTTOM),
        x2: points[i + 1].x,
        y2: PAD_TOP + (1 - b.theta_milli / 1000) * (H - PAD_TOP - PAD_BOTTOM),
      });
    }
  }

  return { points, segments, threshold, width: W, height: H };
}

/** Вторая линия spec-графика — открытые вопросы (пунктир). */
export function oqLine(series: ChartPoint[], top: number): { x: number; y: number }[] {
  const safeTop = Math.max(1, top);
  return series
    .filter((p) => p.measured && p.oq_count !== null)
    .map((p) => ({
      x: xOf(p.round, series.length),
      y: PAD_TOP + (1 - (p.oq_count ?? 0) / safeTop) * (H - PAD_TOP - PAD_BOTTOM),
    }));
}