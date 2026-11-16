import { createEffect, on } from 'solid-js';
import type { RunController } from './run-controller';

/**
 * When this person's own answer or cancel goes through and the Run is read
 * past it, the control they used goes away and focus would fall to the page
 * or the sheet itself; it moves to the Run's status line, which says what
 * happened. A Run that changes on a poll leaves focus where it is.
 */
export function createFocusRescue(
  controller: RunController,
  root: () => HTMLElement | undefined,
  target: () => HTMLElement | undefined
) {
  createEffect(
    on(
      () => [controller.waiting()?.gate, controller.status()?.kind] as const,
      ([gate, kind], previous) => {
        const pending = controller.rescue();
        if (!previous || !pending) return;
        const gone =
          pending === 'answer' ? gate !== previous[0] : kind === 'ended';
        if (!gone) return;
        controller.clearRescue();
        queueMicrotask(() => {
          const active = document.activeElement;
          const lost = !active || active === document.body || active === root();
          if (lost) target()?.focus({ preventScroll: true });
        });
      }
    )
  );
}
