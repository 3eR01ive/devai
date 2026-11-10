// Правило отмены удержания смахиванием (REQ-RESP-06): один порог на все
// экраны, без DOM — чистые координаты.
import { describe, expect, it } from "vitest";

import {
  HOLD_CANCEL_DISTANCE_PX,
  holdMoveCancels,
  holdPoint,
} from "../hold_gesture";

describe("hold gesture cancellation", () => {
  it("uses 40px as the single cancellation threshold", () => {
    expect(HOLD_CANCEL_DISTANCE_PX).toBe(40);
  });

  it("does not cancel a still finger", () => {
    expect(holdMoveCancels({ x: 10, y: 10 }, { x: 10, y: 10 })).toBe(false);
    expect(holdMoveCancels({ x: 10, y: 10 }, { x: 49, y: 10 })).toBe(false);
  });

  it("cancels a swipe past the threshold in any direction", () => {
    expect(holdMoveCancels({ x: 10, y: 10 }, { x: 51, y: 10 })).toBe(true);
    expect(holdMoveCancels({ x: 10, y: 10 }, { x: 10, y: 51 })).toBe(true);
    expect(holdMoveCancels({ x: 10, y: 10 }, { x: -31, y: 10 })).toBe(true);
  });

  it("reads pointer coordinates (and tolerates a missing point in tests)", () => {
    expect(holdPoint({ clientX: 7, clientY: 9 })).toEqual({ x: 7, y: 9 });
    expect(holdPoint({})).toEqual({ x: 0, y: 0 });
  });
});
