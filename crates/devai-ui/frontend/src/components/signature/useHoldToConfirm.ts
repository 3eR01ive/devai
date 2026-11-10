// Машинерия удержания кнопки подписи (REQ-SIGN-04 AC3), вынесенная из
// компонента (rework R4): таймер, pointer capture и кадры rAF живут здесь, а
// корневой компонент только раскладывает кнопки.
//
// Почему так: удержание — единственный путь мутации из UI, и смешивать его с
// рендером абзаца/требований значит прятать инвариант (одиночный и двойной
// клик ничего не применяют) среди разметки. Обработчика `click` здесь нет
// вообще; отпускание раньше HOLD_MS вызывает stop() до emit.
import { onScopeDispose, ref, type Ref } from "vue";

import { HOLD_MS, holdCompletes } from "../../signature_view";
import { holdMoveCancels, holdPoint, type HoldPoint } from "../../hold_gesture";

// The four human gate acts share ONE hold machine: the spec signature's
// approve/reject and the QA gate's accept/return (REQ-QG-02 AC5 — the same
// threshold and the same disabled look as the signature).
export type HoldKind = "approve" | "reject" | "accept" | "return";

export interface HoldToConfirm {
  /** Какая кнопка удерживается сейчас (null — никакая). */
  holding: Ref<HoldKind | null>;
  /** Прогресс удержания 0..1 — только для заливки кнопки. */
  progress: Ref<number>;
  start(kind: HoldKind, event: PointerEvent): void;
  /** Смахивание пальцем отменяет удержание и сбрасывает прогресс. */
  move(event: PointerEvent): void;
  release(): void;
}

/**
 * `canStart` читается в момент нажатия (обоснование могло стать пустым),
 * `onComplete` вызывается ровно один раз — когда удержание пересекло порог.
 */
export function useHoldToConfirm(
  canStart: () => boolean,
  onComplete: (kind: HoldKind) => void,
): HoldToConfirm {
  const holding = ref<HoldKind | null>(null);
  const progress = ref(0);
  let raf = 0;
  let startedAt = 0;
  let origin: HoldPoint | null = null;

  function stop() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    holding.value = null;
    progress.value = 0;
    origin = null;
  }

  function start(kind: HoldKind, event: PointerEvent) {
    if (!canStart()) return;
    stop();
    (event.currentTarget as HTMLElement | null)?.setPointerCapture?.(event.pointerId);
    holding.value = kind;
    origin = holdPoint(event);
    startedAt = performance.now();
    const tick = () => {
      const elapsed = performance.now() - startedAt;
      progress.value = Math.min(1, elapsed / HOLD_MS);
      if (holdCompletes(elapsed)) {
        stop();
        onComplete(kind);
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
  }

  // Смахивание пальцем: сдвиг больше порога отменяет удержание, даже если
  // палец остаётся в границах кнопки (REQ-RESP-06).
  function move(event: PointerEvent) {
    if (!holding.value || !origin) return;
    if (holdMoveCancels(origin, holdPoint(event))) stop();
  }

  // Отпускание раньше порога: мутации нет.
  function release() {
    stop();
  }

  // Уход со страницы посреди удержания не оставляет ни кадра, ни состояния.
  onScopeDispose(stop);

  return { holding, progress, start, move, release };
}
