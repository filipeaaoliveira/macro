import { createSignal, For, type JSX, Show } from 'solid-js';

/** A series a trend chart can plot, with its legend color. */
export type TrendSeries<K extends string> = {
  key: K;
  label: string;
  color: string;
};

/** The legend of a trend chart, each entry a toggle that shows or hides it. */
export function SeriesToggles<K extends string>(props: {
  /** The group's accessible name. */
  label: string;
  series: TrendSeries<K>[];
  visible: K[];
  onToggle: (key: K) => void;
}) {
  return (
    <div class="flex flex-wrap gap-1.5" aria-label={props.label}>
      <For each={props.series}>
        {(series) => (
          <button
            type="button"
            aria-pressed={props.visible.includes(series.key)}
            onClick={() => props.onToggle(series.key)}
            class="inline-flex items-center gap-1.5 rounded-md border border-edge-muted bg-panel px-2 py-1 text-xs text-ink-muted hover:bg-hover focus-visible:outline-2 focus-visible:outline-ink"
            classList={{ 'opacity-40': !props.visible.includes(series.key) }}
          >
            <span
              class="size-2 rounded-full"
              style={{ background: series.color }}
            />
            {series.label}
          </button>
        )}
      </For>
    </div>
  );
}

/** Which keys stay visible after toggling `key`. */
export function toggleSeries<K extends string>(visible: K[], key: K): K[] {
  return visible.includes(key)
    ? visible.filter((shown) => shown !== key)
    : [...visible, key];
}

/**
 * A chart people explore by keyboard: focus picks the last day, the left
 * and right arrows move between days, and the picked day's values are read
 * out below the chart.
 */
export function DayExplorer<D>(props: {
  days: D[];
  /** The group's accessible name, saying how to explore it. */
  label: string;
  /** The picked day's values. */
  status: (day: D) => JSX.Element;
  children: JSX.Element;
}) {
  const [selectedDay, setSelectedDay] = createSignal<number>();
  const activeDay = () => {
    const index = selectedDay();
    if (index === undefined) return undefined;
    return props.days[Math.min(index, props.days.length - 1)];
  };
  return (
    <div
      class="rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ink/20"
      tabIndex={0}
      role="group"
      aria-label={props.label}
      onFocus={() => setSelectedDay(props.days.length - 1)}
      onBlur={() => setSelectedDay(undefined)}
      onKeyDown={(event) => {
        if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
        event.preventDefault();
        setSelectedDay((index) =>
          Math.max(
            0,
            Math.min(
              props.days.length - 1,
              (index ?? props.days.length - 1) +
                (event.key === 'ArrowRight' ? 1 : -1)
            )
          )
        );
      }}
    >
      {props.children}
      <Show when={activeDay()}>
        {(day) => (
          <div
            role="status"
            aria-live="polite"
            class="flex flex-wrap gap-x-4 gap-y-1 rounded-md bg-surface-1 px-3 py-2 text-xs text-ink-muted"
          >
            {props.status(day())}
          </div>
        )}
      </Show>
    </div>
  );
}

/** A trend chart's numbers as a table, folded under a summary. */
export function DailyDataTable<D, K extends string>(props: {
  summary: string;
  caption: string;
  dateHeading: string;
  days: D[];
  date: (day: D) => string;
  columns: TrendSeries<K>[];
  value: (day: D, key: K) => number;
}) {
  return (
    <details class="mt-4 text-xs text-ink-muted">
      <summary class="w-fit rounded py-1 focus-visible:outline-2 focus-visible:outline-ink">
        {props.summary}
      </summary>
      <div class="mt-2 max-h-60 overflow-auto rounded-lg border border-edge-muted">
        <table class="w-full text-left tabular-nums">
          <caption class="sr-only">{props.caption}</caption>
          <thead class="bg-surface-2">
            <tr>
              <th class="p-3">{props.dateHeading}</th>
              <For each={props.columns}>
                {(column) => <th class="p-3 font-medium">{column.label}</th>}
              </For>
            </tr>
          </thead>
          <tbody>
            <For each={props.days}>
              {(day) => (
                <tr class="border-t border-edge-muted">
                  <th class="whitespace-nowrap p-3 font-normal">
                    {props.date(day)}
                  </th>
                  <For each={props.columns}>
                    {(column) => (
                      <td class="p-3">{props.value(day, column.key)}</td>
                    )}
                  </For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </details>
  );
}
