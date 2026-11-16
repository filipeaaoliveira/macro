import type { RunStatus } from '../core/run';
import { RunStatusDot, runStatusDetail } from './run-status';

/**
 * One Run in a task's list: its request, where it is, and when it started.
 * The row geometry follows the agent activity rows beside it.
 */
export function RunRow(props: {
  title: string;
  status: RunStatus;
  time: string;
  onOpen: (event: MouseEvent) => void;
}) {
  return (
    <button
      type="button"
      data-factory-run-row
      class="flex w-full min-w-0 items-start gap-2 rounded-lg px-2 py-1.5 text-left outline-none not-touch:hover:bg-hover focus-visible:outline-2 focus-visible:outline-accent touch:min-h-11"
      onClick={(event) => props.onOpen(event)}
    >
      <span class="flex h-4.5 shrink-0 items-center">
        <RunStatusDot status={props.status} />
      </span>
      <span class="flex min-w-0 flex-1 flex-col">
        <span class="truncate text-[13px] leading-[18px] font-medium text-ink">
          {props.title}
        </span>
        <span class="truncate text-[11.5px] leading-4 text-ink-subtle">
          {runStatusDetail(props.status)}
        </span>
      </span>
      <span class="flex h-4.5 shrink-0 items-center text-[11.5px] tabular-nums text-ink-subtle">
        {props.time}
      </span>
    </button>
  );
}
