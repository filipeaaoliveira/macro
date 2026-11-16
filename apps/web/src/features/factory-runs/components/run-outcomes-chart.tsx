import { formatLocale, t } from '@app/lib/i18n';
import HashIcon from '@icon/system/regular/hash.svg';
import PercentIcon from '@icon/system/regular/percent.svg';
import { ButtonGroup } from '@ui';
import { arc, type PieArcDatum, pie } from 'd3-shape';
import { createMemo, createSignal, For, Show } from 'solid-js';
import {
  INSIGHT_OUTCOMES,
  type InsightOutcome,
  type OutcomeCounts,
  runCount,
} from '../core/insights';
import { insightOutcomeColor, insightOutcomeLabel } from './insight-outcomes';

type Row = { key: InsightOutcome; count: number };

/**
 * How the period's Runs went, as a donut with its legend: each Run in one
 * state. Laid out as Scheduling's event outcomes chart is.
 */
export function RunOutcomesChart(props: { counts: OutcomeCounts }) {
  const rows = (): Row[] =>
    INSIGHT_OUTCOMES.map((key) => ({ key, count: props.counts[key] }));
  const total = () => runCount(props.counts);
  const [active, setActive] = createSignal<InsightOutcome>();
  const [display, setDisplay] = createSignal<'count' | 'percent'>('count');
  const number = (value: number) =>
    new Intl.NumberFormat(formatLocale()).format(value);
  const percentage = (row: Row) =>
    new Intl.NumberFormat(formatLocale(), {
      style: 'percent',
      maximumFractionDigits: 0,
    }).format(total() ? row.count / total() : 0);
  const label = (row: Row) =>
    t('factoryInsights.outcomeShare', {
      outcome: insightOutcomeLabel(row.key),
      count: number(row.count),
      share: percentage(row),
    });
  const selected = () => rows().find((row) => row.key === active());
  const slices = createMemo(() =>
    pie<Row>()
      .sort(null)
      .value((row) => row.count)(rows().filter((row) => row.count > 0))
  );
  const path = arc<PieArcDatum<Row>>()
    .innerRadius(64)
    .outerRadius(94)
    .cornerRadius(3)
    .padAngle((slice) =>
      Math.min(0.025, (slice.endAngle - slice.startAngle) / 3)
    );

  return (
    <div class="flex min-w-0 flex-col items-center gap-5 p-5 @xl/insight-main:flex-row">
      <div class="relative size-48 shrink-0">
        <svg
          viewBox="-100 -100 200 200"
          class="size-full"
          role="group"
          aria-label={t('factoryInsights.outcomesChart')}
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
                  fill={insightOutcomeColor(slice.data.key)}
                  class="outline-none focus-visible:stroke-ink focus-visible:stroke-2"
                  opacity={active() && active() !== slice.data.key ? 0.4 : 1}
                  tabIndex={0}
                  role="img"
                  aria-label={label(slice.data)}
                  onPointerEnter={() => setActive(slice.data.key)}
                  onFocus={() => setActive(slice.data.key)}
                  onBlur={() => setActive(undefined)}
                >
                  <title>{label(slice.data)}</title>
                </path>
              )}
            </For>
          </Show>
          <text text-anchor="middle" class="pointer-events-none fill-ink">
            <tspan x="0" y="0" class="text-3xl font-semibold tabular-nums">
              {number(total())}
            </tspan>
            <tspan x="0" y="22" class="fill-ink-muted text-xs">
              {t('factoryInsights.totalRuns')}
            </tspan>
          </text>
        </svg>
      </div>
      <div class="min-w-0 w-full flex-1">
        <div class="mb-4 flex justify-end">
          <ButtonGroup finish="raised" variant="secondary">
            <ButtonGroup.Button
              aria-label={t('factoryInsights.showCounts')}
              tooltip={t('factoryInsights.showCounts')}
              type="button"
              size="icon-md"
              aria-pressed={display() === 'count'}
              onClick={() => setDisplay('count')}
            >
              <HashIcon class="size-4" />
            </ButtonGroup.Button>
            <ButtonGroup.Button
              aria-label={t('factoryInsights.showPercentages')}
              tooltip={t('factoryInsights.showPercentages')}
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
          aria-label={t('factoryInsights.outcomes')}
          class="grid gap-x-5 gap-y-3 @xl/insight-main:grid-cols-2"
        >
          <For each={rows()}>
            {(row) => (
              <li class="flex min-w-0 items-center gap-2 text-xs">
                <span
                  class="size-2.5 shrink-0 rounded-full"
                  style={{ background: insightOutcomeColor(row.key) }}
                />
                <span class="flex-1 text-ink-muted">
                  {insightOutcomeLabel(row.key)}
                </span>
                <span class="min-w-9 shrink-0 text-right font-medium tabular-nums">
                  {display() === 'count' ? number(row.count) : percentage(row)}
                </span>
              </li>
            )}
          </For>
        </ul>
        <p role="status" class="mt-4 min-h-4 text-xs text-ink-muted">
          {selected()
            ? label(selected()!)
            : total()
              ? t('factoryInsights.oneStatePerRun')
              : t('factoryInsights.noRunsInPeriod')}
        </p>
      </div>
    </div>
  );
}
