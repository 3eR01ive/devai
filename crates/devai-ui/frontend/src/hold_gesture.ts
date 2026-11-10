// Общее правило отмены удержания смахиванием для тач-экрана (REQ-RESP-06):
// палец — не мышь, и удержание обязано отменяться, если палец сдвинулся от
// точки нажатия, даже когда он остаётся в границах кнопки. Порог — одна
// именованная константа на все экраны, отдельного мобильного механизма
// подписи не вводится.
//
// Чистые функции без DOM: их проверяют тесты (src/__tests__ и tests/), а
// useHoldToConfirm и экран «Связка модели» только передают координаты.

/** Сдвиг пальца больше этого числа пикселей отменяет удержание. */
export const HOLD_CANCEL_DISTANCE_PX = 40;

export interface HoldPoint {
  x: number;
  y: number;
}

/// Точка нажатия из pointer-события. В тестовом окружении координат может
/// не быть — тогда точка считается (0,0), а отмена работает по сдвигу.
export function holdPoint(event: { clientX?: number; clientY?: number }): HoldPoint {
  return { x: event.clientX ?? 0, y: event.clientY ?? 0 };
}

/// Удержание отменяется, когда палец ушёл от точки нажатия дальше порога.
export function holdMoveCancels(
  origin: HoldPoint,
  current: HoldPoint,
  threshold = HOLD_CANCEL_DISTANCE_PX,
): boolean {
  return Math.hypot(current.x - origin.x, current.y - origin.y) > threshold;
}
