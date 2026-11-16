import HashIcon from '@icon/system/regular/hash.svg';
import PercentIcon from '@icon/system/regular/percent.svg';
import { ButtonGroup } from '@ui';
import { arc, type PieArcDatum, pie } from 'd3-shape';
import { createMemo, createSignal, For, Show } from 'solid-js';
import type { InsightOutcomeKey } from '../core/insight-outcomes';

const BOOKING_COLORS: Record<string, string | undefined> = {
  completed: 'var(--color-success)',
  upcoming: 'var(--color-purple)',
  cancelled: 'var(--color-failure)',
  pendingApproval: 'var(--color-blue)',
  hostNoShow: 'var(--color-ink-muted)',
  guestNoShow: 'var(--color-warning)',
} satisfies Record<InsightOutcomeKey, string>;

/** One exclusive outcome and how many items reached it. */
export type OutcomeRow<K extends string> = {
  key: K;
  label: string;
  count: number;
};

/**
 * Exclusive outcomes as a donut with a legend. D3 computes the slices; the
 * renderer has no booking or aggregation policy. The words and colors
 * default to a booking's outcomes; other insights pass their own.
 */
export function EventOutcomesChart<K extends string>(props: {
  rows: OutcomeRow<K>[];
  /** Each outcome's color; booking outcomes have theirs by default. */
  colors?: Record<K, string>;
  /** Under the total, inside the donut. */
  totalLabel?: string;
  /** The donut's accessible name. */
  chartLabel?: string;
  /** The legend's accessible name. */
  listLabel?: string;
  /** Under the legend when nothing is counted. */
  emptyText?: string;
  /** Under the legend when nothing is pointed at. */
  hintText?: string;
  showCountsLabel?: string;
  showPercentagesLabel?: string;
  /** The locale numbers are written in; the browser's by default. */
  locale?: string;
  /** A slice's description; `Completed: 11 (79%)` by default. */
  describe?: (row: OutcomeRow<K>, count: string, share: string) => string;
}) {
  const total = () => props.rows.reduce((sum, row) => sum + row.count, 0);
  const [active, setActive] = createSignal<K>();
  const [display, setDisplay] = createSignal<'count' | 'percent'>('count');
  const selected = () => props.rows.find((row) => row.key === active());
  const color = (key: K): string =>
    props.colors?.[key] ?? BOOKING_COLORS[key] ?? 'var(--color-ink-muted)';
  const slices = createMemo(() =>
    pie<OutcomeRow<K>>()
      .sort(null)
      .value((row) => row.count)(props.rows.filter((row) => row.count > 0))
  );
  const path = arc<PieArcDatum<OutcomeRow<K>>>()
    .innerRadius(64)
    .outerRadius(94)
    .cornerRadius(3)
    .padAngle((slice) =>
      Math.min(0.025, (slice.endAngle - slice.startAngle) / 3)
    );
  const count = (value: number) => value.toLocaleString(props.locale);
  const percentage = (row: OutcomeRow<K>) => {
    const share = total() ? row.count / total() : 0;
    return props.locale
      ? new Intl.NumberFormat(props.locale, {
          style: 'percent',
          maximumFractionDigits: 0,
        }).format(share)
      : `${Math.round(share * 100)}%`;
  };
  const label = (row: OutcomeRow<K>) =>
    props.describe
      ? props.describe(row, count(row.count), percentage(row))
      : `${row.label}: ${count(row.count)} (${percentage(row)})`;
  return (
    <div class="flex min-w-0 flex-col items-center gap-5 p-5 @xl/insight-main:flex-row">
      <div class="relative size-48 shrink-0">
        <svg
          viewBox="-100 -100 200 200"
          class="size-full"
          role="group"
          aria-label={props.chartLabel ?? 'Event outcomes donut chart'}
          onPointerLeave={() => setActive(undefined)}
        >
          <Show
            when={total() > 0}
            fallback={
              <circle
                r="79"
                fill="none"
                stroke="var(--color-edge-muted)"
                stroke-width="30"
              />
            }
          >
            <For each={slices()}>
              {(slice) => (
                <path
                  data-outcome={slice.data.key}
                  d={path(slice) ?? ''}
                  fill={color(slice.data.key)}
                  class="outline-none focus-visible:stroke-ink focus-visible:stroke-2"
                  opacity={active() && active() !== slice.data.key ? 0.4 : 1}
                  tabIndex={0}
                  role="img"
                  aria-label={label(slice.data)}
                  onPointerEnter={() => setActive(() => slice.data.key)}
                  onFocus={() => setActive(() => slice.data.key)}
                  onBlur={() => setActive(undefined)}
                >
                  <title>{label(slice.data)}</title>
                </path>
              )}
            </For>
          </Show>
          <text text-anchor="middle" class="pointer-events-none fill-ink">
            <tspan x="0" y="0" class="text-3xl font-semibold tabular-nums">
              {count(total())}
            </tspan>
            <tspan x="0" y="22" class="fill-ink-muted text-xs">
              {props.totalLabel ?? 'Total events'}
            </tspan>
          </text>
        </svg>
      </div>
      <div class="min-w-0 w-full flex-1">
        <div class="mb-4 flex justify-end">
          <ButtonGroup finish="raised" variant="secondary">
            <ButtonGroup.Button
              aria-label={props.showCountsLabel ?? 'Show counts'}
              tooltip={props.showCountsLabel ?? 'Show counts'}
              type="button"
              size="icon-md"
              aria-pressed={display() === 'count'}
              onClick={() => setDisplay('count')}
            >
              <HashIcon class="size-4" />
            </ButtonGroup.Button>
            <ButtonGroup.Button
              aria-label={props.showPercentagesLabel ?? 'Show percentages'}
              tooltip={props.showPercentagesLabel ?? 'Show percentages'}
              type="button"
              size="icon-md"
              aria-pressed={display() === 'percent'}
              onClick={() => setDisplay('percent')}
            >
              <PercentIcon class="size-4" />
            </ButtonGroup.Button>
          </ButtonGroup>
        </div>
        <ul
          aria-label={props.listLabel ?? 'Event outcomes'}
          class="grid gap-x-5 gap-y-3 @xl/insight-main:grid-cols-2"
        >
          <For each={props.rows}>
            {(row) => (
              <li class="flex min-w-0 items-center gap-2 text-xs">
                <span
                  class="size-2.5 shrink-0 rounded-full"
                  style={{ background: color(row.key) }}
                />
                <span class="flex-1 text-ink-muted">{row.label}</span>
                <span class="min-w-9 shrink-0 text-right font-medium tabular-nums">
                  {display() === 'count' ? count(row.count) : percentage(row)}
                </span>
              </li>
            )}
          </For>
        </ul>
        <p role="status" class="mt-4 min-h-4 text-xs text-ink-muted">
          {selected()
            ? label(selected()!)
            : total()
              ? (props.hintText ?? 'Each booking appears in one outcome.')
              : (props.emptyText ?? 'No events in this period.')}
        </p>
      </div>
    </div>
  );
}
