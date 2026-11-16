import { For, type JSX } from 'solid-js';
import type { RunLink } from '../core/insights';
import { formatRunDate } from './insight-format';
import { insightOutcomeColor, insightOutcomeLabel } from './insight-outcomes';

/** Runs to open, each with its state, title, date and a trailing figure. */
export function RunLinkList(props: {
  label: string;
  runs: { run: RunLink; detail: JSX.Element }[];
  onOpenRun: (runId: string) => void;
}) {
  return (
    <ul aria-label={props.label} class="-mx-2 flex flex-col">
      <For each={props.runs}>
        {(item) => (
          <li>
            <button
              type="button"
              class="flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm not-touch:hover:bg-hover focus-visible:outline-2 focus-visible:outline-ink touch:min-h-11"
              onClick={() => props.onOpenRun(item.run.id)}
            >
              <span
                class="size-2 shrink-0 rounded-full"
                style={{
                  background: insightOutcomeColor(
                    item.run.outcome ?? 'running'
                  ),
                }}
              />
              <span class="sr-only">
                {insightOutcomeLabel(item.run.outcome ?? 'running')}
              </span>
              <span class="min-w-0 flex-1 truncate">{item.run.title}</span>
              <span class="shrink-0 text-xs text-ink-muted tabular-nums">
                {item.detail}
              </span>
              <span class="shrink-0 text-xs text-ink-muted">
                {formatRunDate(item.run.createdAt)}
              </span>
            </button>
          </li>
        )}
      </For>
    </ul>
  );
}
