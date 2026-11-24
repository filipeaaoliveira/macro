import { TIP } from '@app/features/scheduling/components/insight-chart-options';
import {
  InsightPlot,
  type InsightPlotModule,
} from '@app/features/scheduling/components/insight-plot';
import { formatLocale, t } from '@app/lib/i18n';
import { fireAndForget } from '@core/util/fireAndForget';
import { buildSimpleEntityUrl } from '@core/util/url';
import ArrowDownIcon from '@icon/system/regular/arrow-down.svg';
import ArrowUpIcon from '@icon/system/regular/arrow-up.svg';
import type { PlotOptions } from '@observablehq/plot';
import { Badge, Button, cn } from '@ui';
import { createSignal, For, type JSX, Show } from 'solid-js';
import { repositoryName } from '../core/board';
import { flowWordLabel } from '../core/flow-words';
import {
  daysWithData,
  groupCounts,
  type Kpi,
  type Kpis,
  OUTCOME_GROUP_COLORS,
  OUTCOME_GROUPS,
  type OutcomeGroup,
  type ParetoPoint,
  type RoleInsight,
  type StepTime,
} from '../core/insight-overview';
import type { EntryInsight, InsightDay, OutcomeCounts } from '../core/insights';
import type { QualityInsights } from '../core/quality';
import { formatInsightDay } from './insight-day';
import { formatInsightDuration, formatTokens } from './insight-format';

/** A panel of the overview: a title, a line on what it shows, its body. */
export function OverviewCard(props: {
  title: string;
  description?: string;
  actions?: JSX.Element;
  class?: string;
  children: JSX.Element;
}) {
  return (
    <section
      aria-label={props.title}
      class={cn(
        'flex min-w-0 flex-col rounded-2xl border border-edge-muted bg-surface-1',
        props.class
      )}
    >
      <header class="flex flex-wrap items-start gap-x-4 gap-y-2 px-5 pt-4 pb-3">
        <div class="flex min-w-0 flex-1 flex-col gap-0.5">
          <h2 class="text-[14px] leading-5 font-semibold text-ink">
            {props.title}
          </h2>
          <Show when={props.description}>
            <p class="text-xs leading-4 text-ink-muted">{props.description}</p>
          </Show>
        </div>
        {props.actions}
      </header>
      <div class="flex min-w-0 flex-1 flex-col px-5 pb-5">{props.children}</div>
    </section>
  );
}

/** Says a panel has nothing to draw yet, and what would fill it. */
export function OverviewEmpty(props: { title: string; hint: string }) {
  return (
    <div class="flex flex-1 flex-col items-center justify-center gap-1 rounded-xl border border-dashed border-edge-muted px-6 py-8 text-center">
      <p class="text-[13px] font-medium text-ink">{props.title}</p>
      <p class="max-w-sm text-xs text-ink-muted">{props.hint}</p>
    </div>
  );
}

/** A figure's days as a small line, the last day marked. */
function Sparkline(props: { values: (number | undefined)[] }) {
  const width = 96;
  const height = 28;
  const points = () => {
    const known = props.values
      .map((value, index) => ({ value, index }))
      .filter((point): point is { value: number; index: number } =>
        Number.isFinite(point.value)
      );
    if (known.length < 2) return [];
    const max = Math.max(...known.map((point) => point.value));
    const min = Math.min(...known.map((point) => point.value));
    const span = max - min || 1;
    const last = Math.max(1, props.values.length - 1);
    return known.map((point) => ({
      x: (point.index / last) * (width - 4) + 2,
      y: height - 3 - ((point.value - min) / span) * (height - 6),
    }));
  };
  const path = () =>
    points()
      .map((point, index) => `${index === 0 ? 'M' : 'L'}${point.x},${point.y}`)
      .join(' ');
  const area = () => {
    const list = points();
    const first = list[0];
    const last = list.at(-1);
    if (!first || !last) return '';
    return `${path()} L${last.x},${height} L${first.x},${height} Z`;
  };
  return (
    <Show when={points().length > 1}>
      <svg
        aria-hidden="true"
        viewBox={`0 0 ${width} ${height}`}
        class="h-7 w-24 shrink-0 overflow-visible text-accent"
      >
        <path d={area()} fill="currentColor" opacity="0.1" />
        <path
          d={path()}
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linejoin="round"
          stroke-linecap="round"
        />
        <Show when={points().at(-1)}>
          {(last) => (
            <circle
              cx={last().x}
              cy={last().y}
              r="2.5"
              fill="currentColor"
              stroke="var(--color-surface-1)"
              stroke-width="1.5"
            />
          )}
        </Show>
      </svg>
    </Show>
  );
}

/** How a figure moved since the period before, coloured by whether that is good. */
function Delta(props: {
  kpi: Kpi;
  lowerIsBetter?: boolean;
  points?: boolean;
  format: (value: number) => string;
}) {
  const change = () => {
    const previous = props.kpi.previous;
    if (previous === undefined) return undefined;
    if (props.points) return props.kpi.value - previous;
    if (previous === 0) return undefined;
    return ((props.kpi.value - previous) / previous) * 100;
  };
  const good = () => {
    const value = change() ?? 0;
    return props.lowerIsBetter ? value < 0 : value > 0;
  };
  return (
    <Show
      when={change() !== undefined}
      fallback={<span class="h-[18px]" aria-hidden="true" />}
    >
      <span class="flex items-center gap-1 text-[11px]">
        <Show
          when={Math.abs(change() ?? 0) >= 0.5}
          fallback={
            <span class="text-ink-subtle">{t('factoryInsights.noChange')}</span>
          }
        >
          <span
            class={cn(
              'inline-flex items-center gap-0.5 rounded-full px-1.5 py-px font-medium tabular-nums',
              good()
                ? 'bg-success-bg text-success-ink'
                : 'bg-failure-bg text-failure-ink'
            )}
          >
            <Show
              when={(change() ?? 0) > 0}
              fallback={<ArrowDownIcon class="size-3" />}
            >
              <ArrowUpIcon class="size-3" />
            </Show>
            {props.points
              ? t('factoryInsights.deltaPoints', {
                  value: Math.abs(Math.round(change() ?? 0)),
                })
              : `${Math.abs(Math.round(change() ?? 0))}%`}
          </span>
          <span class="text-ink-subtle">
            {t('factoryInsights.vsPrevious', {
              value: props.format(props.kpi.previous ?? 0),
            })}
          </span>
        </Show>
      </span>
    </Show>
  );
}

function KpiTile(props: {
  label: string;
  note?: string;
  kpi: Kpi;
  format: (value: number) => string;
  lowerIsBetter?: boolean;
  points?: boolean;
}) {
  return (
    <div class="flex min-w-[170px] flex-1 flex-col gap-2 px-5 py-4">
      <span class="flex items-baseline justify-between gap-2 text-xs font-medium text-ink-muted">
        {props.label}
        <Show when={props.note}>
          <span class="font-normal text-ink-subtle">{props.note}</span>
        </Show>
      </span>
      <div class="flex items-end justify-between gap-3">
        <span class="text-[28px] leading-8 font-semibold tracking-tight text-ink">
          {props.format(props.kpi.value)}
        </span>
        <Show when={props.kpi.trend}>
          {(trend) => <Sparkline values={trend()} />}
        </Show>
      </div>
      <Delta
        kpi={props.kpi}
        lowerIsBetter={props.lowerIsBetter}
        points={props.points}
        format={props.format}
      />
    </div>
  );
}

const integer = (value: number) =>
  new Intl.NumberFormat(formatLocale()).format(Math.round(value));
const percent = (value: number) => `${Math.round(value)}%`;

/** The headline figures, each with its days and its change. */
export function KpiStrip(props: { kpis: Kpis; retried?: number }) {
  return (
    <div class="flex flex-wrap divide-edge-muted overflow-hidden rounded-2xl border border-edge-muted bg-surface-1 max-sm:divide-y sm:divide-x">
      <KpiTile
        label={t('factoryInsights.totalRuns')}
        note={
          props.retried
            ? t('factoryInsights.retriedNote', { count: props.retried })
            : undefined
        }
        kpi={props.kpis.runs}
        format={integer}
      />
      <Show when={props.kpis.merged}>
        {(kpi) => (
          <KpiTile
            label={t('factoryInsights.mergedRuns')}
            kpi={kpi()}
            format={integer}
          />
        )}
      </Show>
      <Show when={props.kpis.successRate}>
        {(kpi) => (
          <KpiTile
            label={t('factoryInsights.successRate')}
            kpi={kpi()}
            format={percent}
            points
          />
        )}
      </Show>
      <Show when={props.kpis.medianRunSeconds}>
        {(kpi) => (
          <KpiTile
            label={t('factoryInsights.medianRunTime')}
            kpi={kpi()}
            format={formatInsightDuration}
            lowerIsBetter
          />
        )}
      </Show>
      <Show when={props.kpis.timeToMergeSeconds}>
        {(kpi) => (
          <KpiTile
            label={t('factoryInsights.timeToMerge')}
            note={t('factoryInsights.median')}
            kpi={kpi()}
            format={formatInsightDuration}
            lowerIsBetter
          />
        )}
      </Show>
      <Show when={props.kpis.autonomy}>
        {(kpi) => (
          <KpiTile
            label={t('factoryInsights.autonomy')}
            kpi={kpi()}
            format={percent}
            points
          />
        )}
      </Show>
      <Show when={props.kpis.costPerMerge}>
        {(kpi) => (
          <KpiTile
            label={t('factoryInsights.costPerMerge')}
            kpi={kpi()}
            format={(value) =>
              kpi().kind === 'money'
                ? `$${value.toFixed(2)}`
                : formatTokens(value)
            }
            lowerIsBetter
          />
        )}
      </Show>
    </div>
  );
}

export function outcomeGroupLabel(group: OutcomeGroup): string {
  switch (group) {
    case 'merged':
      return t('factoryInsights.groupMerged');
    case 'failed':
      return t('factoryInsights.groupFailed');
    case 'handedOff':
      return t('factoryInsights.groupHandedOff');
    case 'cancelled':
      return t('factoryInsights.groupCancelled');
    case 'running':
      return t('factoryInsights.groupRunning');
  }
}

function runsOverTimePlot(
  Plot: InsightPlotModule,
  width: number,
  days: InsightDay[]
): PlotOptions {
  const data = days.flatMap((day) => {
    const counts = groupCounts(day.counts);
    const tip = [
      formatInsightDay(day.date, true),
      ...[...OUTCOME_GROUPS]
        .reverse()
        .filter((group) => counts[group] > 0)
        .map((group) => `${outcomeGroupLabel(group)}: ${counts[group]}`),
    ].join('\n');
    return OUTCOME_GROUPS.map((group) => ({
      date: new Date(`${day.date}T00:00:00Z`),
      count: counts[group],
      group: outcomeGroupLabel(group),
      tip,
    }));
  });
  const most = Math.max(
    4,
    ...days.map((day) =>
      Object.values(groupCounts(day.counts)).reduce((a, b) => a + b, 0)
    )
  );
  const labelEvery = Math.max(
    1,
    Math.ceil(days.length / Math.max(2, Math.floor(width / 90)))
  );
  return {
    width,
    height: 260,
    marginLeft: 32,
    marginRight: 8,
    marginBottom: 28,
    marginTop: 8,
    x: {
      type: 'band',
      label: null,
      tickSize: 0,
      padding: days.length > 45 ? 0.12 : 0.18,
      tickFormat: (date: Date) =>
        formatInsightDay(date.toISOString().slice(0, 10)),
      ticks: days
        .filter((_, index) => index % labelEvery === 0)
        .map((day) => new Date(`${day.date}T00:00:00Z`)),
    },
    y: {
      label: null,
      grid: true,
      domain: [0, most],
      ticks: 4,
      tickFormat: (value: number) =>
        Number.isInteger(value) ? integer(value) : '',
    },
    color: {
      domain: OUTCOME_GROUPS.map(outcomeGroupLabel),
      range: OUTCOME_GROUPS.map((group) => OUTCOME_GROUP_COLORS[group]),
      legend: false,
    },
    marks: [
      Plot.barY(
        data,
        Plot.stackY({
          x: 'date',
          y: 'count',
          fill: 'group',
          order: OUTCOME_GROUPS.map(outcomeGroupLabel),
          insetTop: 1,
          insetBottom: 1,
          rx: 2,
          title: 'tip',
          tip: TIP,
        })
      ),
      Plot.ruleY([0], { stroke: 'var(--color-edge)' }),
    ],
  };
}

/**
 * The period's Runs by day, stacked by how they went in the dashboard's
 * tones, with each tone's total and share as the legend.
 */
export function RunsOverTime(props: {
  days: InsightDay[];
  totals: OutcomeCounts;
}) {
  const totals = () => groupCounts(props.totals);
  // The axis starts at the first day a Run started on.
  const shown = () => daysWithData(props.days);
  const all = () => Object.values(totals()).reduce((a, b) => a + b, 0);
  return (
    <OverviewCard
      title={t('factoryInsights.runsOverTime')}
      description={
        shown()[0] && shown().length < props.days.length
          ? t('factoryInsights.runsOverTimeSince', {
              date: formatInsightDay(shown()[0]?.date ?? ''),
            })
          : t('factoryInsights.runsOverTimeDescription')
      }
      class="lg:col-span-2"
    >
      <ul
        class="mb-3 flex flex-wrap gap-x-5 gap-y-1.5"
        aria-label={t('factoryInsights.outcomeGroups')}
      >
        <For
          each={[...OUTCOME_GROUPS]
            .reverse()
            .filter((group) => totals()[group] > 0)}
        >
          {(group) => (
            <li class="flex items-center gap-2 text-xs">
              <span
                class="size-2.5 rounded-sm"
                style={{ background: OUTCOME_GROUP_COLORS[group] }}
              />
              <span class="text-ink-muted">{outcomeGroupLabel(group)}</span>
              <span class="font-semibold tabular-nums text-ink">
                {integer(totals()[group])}
              </span>
              <span class="tabular-nums text-ink-subtle">
                {all() > 0 ? percent((totals()[group] / all()) * 100) : ''}
              </span>
            </li>
          )}
        </For>
      </ul>
      <Show
        when={all() > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noRunsInPeriod')}
            hint={t('factoryInsights.noRunsHint')}
          />
        }
      >
        <InsightPlot
          label={t('factoryInsights.runsOverTimeChart')}
          height={260}
          options={(plot, width) => runsOverTimePlot(plot, width, shown())}
        />
      </Show>
    </OverviewCard>
  );
}

function stepTimePlot(
  Plot: InsightPlotModule,
  width: number,
  steps: StepTime[]
): PlotOptions {
  const rows = steps.map((step) => ({
    ...step,
    label: flowWordLabel(step.entry),
    tip: [
      flowWordLabel(step.entry),
      t('factoryInsights.stepTimeTipMedian', {
        time: formatInsightDuration(step.medianSeconds),
      }),
      step.p90Seconds
        ? t('factoryInsights.stepTimeTipSlow', {
            time: formatInsightDuration(step.p90Seconds),
          })
        : '',
      t('factoryInsights.stepTimeTipVisits', {
        visits: step.visits,
        runs: step.runs,
      }),
      step.waitSeconds
        ? t('factoryInsights.stepTimeTipWait', {
            time: formatInsightDuration(step.waitSeconds),
          })
        : '',
    ]
      .filter(Boolean)
      .join('\n'),
  }));
  const most = Math.max(
    ...steps.map((step) => step.p90Seconds ?? step.medianSeconds)
  );
  return {
    width,
    height: Math.max(120, rows.length * 26 + 30),
    marginLeft: 92,
    marginRight: 72,
    marginTop: 4,
    marginBottom: 22,
    x: {
      label: null,
      grid: true,
      domain: [0, most * 1.05],
      ticks: Math.max(2, Math.floor(width / 140)),
      tickFormat: (value: number) => formatInsightDuration(value),
    },
    y: {
      label: null,
      domain: rows.map((row) => row.label),
      tickSize: 0,
      padding: 0.35,
    },
    marks: [
      Plot.ruleY(rows, {
        y: 'label',
        x1: 'medianSeconds',
        x2: (row: StepTime) => row.p90Seconds ?? row.medianSeconds,
        stroke: 'var(--color-ink-extra-muted)',
        strokeWidth: 1.5,
      }),
      Plot.barX(rows, {
        y: 'label',
        x: 'medianSeconds',
        fill: (row: StepTime) =>
          row.agent ? 'var(--color-accent)' : 'var(--color-ink-disabled)',
        rx: 3,
        title: 'tip',
        tip: TIP,
      }),
      Plot.text(rows, {
        y: 'label',
        x: 'medianSeconds',
        text: (row: StepTime) => formatInsightDuration(row.medianSeconds),
        textAnchor: 'end',
        dx: -6,
        fill: 'white',
        fontWeight: 600,
        filter: (row: StepTime) => (row.medianSeconds / most) * width > 110,
      }),
      Plot.text(rows, {
        y: 'label',
        x: 'medianSeconds',
        text: (row: StepTime) => formatInsightDuration(row.medianSeconds),
        textAnchor: 'start',
        dx: 6,
        dy: -8,
        fill: 'var(--color-ink-muted)',
        filter: (row: StepTime) => (row.medianSeconds / most) * width <= 110,
      }),
      Plot.text(rows, {
        y: 'label',
        text: (row: StepTime) => `${row.visits}×`,
        frameAnchor: 'right',
        textAnchor: 'end',
        dx: 64,
        fill: 'var(--color-ink-muted)',
      }),
    ],
  };
}

/**
 * Where the Runs' time goes: each Step's median time a visit as a bar,
 * reaching on to its slow tenth, with its visits beside it.
 */
export function StepTimeChart(props: { steps: StepTime[] }) {
  return (
    <OverviewCard
      title={t('factoryInsights.timeByStep')}
      description={t('factoryInsights.timeByStepDescription')}
      class="lg:col-span-2"
    >
      <Show
        when={props.steps.length > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noStepTimes')}
            hint={t('factoryInsights.noRunsHint')}
          />
        }
      >
        <InsightPlot
          label={t('factoryInsights.timeByStepChart')}
          height={Math.max(120, props.steps.length * 26 + 30)}
          options={(plot, width) => stepTimePlot(plot, width, props.steps)}
        />
        <p class="mt-2 flex items-center gap-4 text-[11px] text-ink-subtle">
          <span class="flex items-center gap-1.5">
            <span class="h-2 w-4 rounded-sm bg-accent" />
            {t('factoryInsights.agentSteps')}
          </span>
          <span class="flex items-center gap-1.5">
            <span class="h-2 w-4 rounded-sm bg-ink-disabled" />
            {t('factoryInsights.factorySteps')}
          </span>
          <span class="flex items-center gap-1.5">
            <span class="h-px w-4 bg-ink-extra-muted" />
            {t('factoryInsights.slowTenth')}
          </span>
          <span class="ml-auto">{t('factoryInsights.visitsColumn')}</span>
        </p>
      </Show>
    </OverviewCard>
  );
}

function roleLabel(role: string): string {
  return role === 'reviewer'
    ? t('factoryInsights.reviewPanel')
    : flowWordLabel(role);
}

/** A share as a bar: green when most passed, amber below four in five. */
function PassMeter(props: { share: number }) {
  return (
    <span class="h-1.5 w-full overflow-hidden rounded-full bg-edge-muted">
      <span
        class={cn(
          'block h-full rounded-full',
          props.share >= 0.8 ? 'bg-success' : 'bg-warning'
        )}
        style={{ width: `${Math.round(props.share * 100)}%` }}
      />
    </span>
  );
}

/** One card per Role: how often its Steps passed, its rounds and its cost. */
export function RoleCards(props: { roles: RoleInsight[] }) {
  return (
    <OverviewCard
      title={t('factoryInsights.roles')}
      description={t('factoryInsights.rolesDescription')}
      class="lg:col-span-3"
    >
      <Show
        when={props.roles.length > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noRoles')}
            hint={t('factoryInsights.noRunsHint')}
          />
        }
      >
        <ul class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-6">
          <For each={props.roles}>
            {(role) => (
              <li class="flex flex-col gap-3 rounded-xl border border-edge-muted bg-surface p-4">
                <div class="flex items-baseline justify-between gap-2">
                  <span class="truncate text-[13px] font-semibold text-ink">
                    {roleLabel(role.role)}
                  </span>
                  <span class="shrink-0 text-[11px] tabular-nums text-ink-subtle">
                    {t('factoryInsights.roleSteps', { count: role.steps })}
                  </span>
                </div>
                <div class="flex flex-col gap-1.5">
                  <div class="flex items-baseline justify-between">
                    <span class="text-xs text-ink-muted">
                      {t('factoryInsights.passRate')}
                    </span>
                    <span class="text-lg leading-6 font-semibold tabular-nums text-ink">
                      {role.passRate === undefined
                        ? t('factoryInsights.notJudged')
                        : percent(role.passRate * 100)}
                    </span>
                  </div>
                  <Show when={role.passRate !== undefined}>
                    <PassMeter share={role.passRate ?? 0} />
                  </Show>
                </div>
                <dl class="grid grid-cols-2 gap-x-3 gap-y-1 text-xs">
                  <dt class="text-ink-muted">{t('factoryInsights.rounds')}</dt>
                  <dd class="text-right tabular-nums text-ink">
                    {(Math.round(role.roundsPerRun * 10) / 10).toLocaleString(
                      formatLocale()
                    )}
                  </dd>
                  <Show when={role.medianSeconds}>
                    {(seconds) => (
                      <>
                        <dt class="text-ink-muted">
                          {t('factoryInsights.medianTime')}
                        </dt>
                        <dd class="text-right tabular-nums text-ink">
                          {formatInsightDuration(seconds())}
                        </dd>
                      </>
                    )}
                  </Show>
                  <Show when={role.blockersPerReview !== undefined}>
                    <dt class="text-ink-muted">
                      {t('factoryInsights.blockersPerReview')}
                    </dt>
                    <dd class="text-right tabular-nums text-ink">
                      {(role.blockersPerReview ?? 0).toFixed(2)}
                    </dd>
                  </Show>
                  <Show
                    when={role.usdPerRun}
                    fallback={
                      <Show when={role.tokensPerRun}>
                        {(tokens) => (
                          <>
                            <dt class="text-ink-muted">
                              {t('factoryInsights.tokensPerRun')}
                            </dt>
                            <dd class="text-right tabular-nums text-ink">
                              {formatTokens(tokens())}
                            </dd>
                          </>
                        )}
                      </Show>
                    }
                  >
                    {(usd) => (
                      <>
                        <dt class="text-ink-muted">
                          {t('factoryInsights.costPerRun')}
                        </dt>
                        <dd class="text-right tabular-nums text-ink">
                          {`$${usd().toFixed(2)}`}
                        </dd>
                      </>
                    )}
                  </Show>
                </dl>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </OverviewCard>
  );
}

function paretoPlot(
  Plot: InsightPlotModule,
  width: number,
  points: ParetoPoint[]
): PlotOptions {
  const frontier = points
    .filter((point) => point.frontier)
    .sort((a, b) => a.tokensPerTrial - b.tokensPerTrial);
  const rows = points.map((point) => ({
    ...point,
    tip: [
      point.label,
      t('factoryInsights.paretoTipScore', { score: point.score.toFixed(2) }),
      t('factoryInsights.paretoTipCost', {
        tokens: formatTokens(point.tokensPerTrial),
      }),
      point.frontier ? t('factoryEvaluation.paretoSet') : '',
    ]
      .filter(Boolean)
      .join('\n'),
  }));
  return {
    width,
    height: 260,
    marginLeft: 40,
    marginRight: 40,
    marginBottom: 36,
    marginTop: 16,
    x: {
      label: null,
      inset: 24,
      grid: true,
      nice: true,
      tickFormat: (value: number) => formatTokens(value),
    },
    y: {
      label: null,
      grid: true,
      inset: 16,
      ticks: 5,
      tickFormat: (value: number) => value.toFixed(2),
    },
    marks: [
      Plot.line(frontier, {
        x: 'tokensPerTrial',
        y: 'score',
        stroke: 'var(--color-success)',
        strokeWidth: 2,
        strokeOpacity: 0.7,
      }),
      Plot.dot(rows, {
        x: 'tokensPerTrial',
        y: 'score',
        r: 5,
        fill: (row: ParetoPoint) =>
          row.frontier
            ? 'var(--color-success)'
            : 'var(--color-ink-extra-muted)',
        stroke: 'var(--color-surface-1)',
        strokeWidth: 2,
        title: 'tip',
        tip: TIP,
      }),
      Plot.text(
        rows.filter((row) => row.frontier),
        {
          x: 'tokensPerTrial',
          y: 'score',
          text: 'label',
          dy: -12,
          fill: 'var(--color-ink)',
          fontWeight: 500,
        }
      ),
    ],
  };
}

/**
 * Cost against quality: each Benchmark suite by its tokens a trial and its
 * mean score, the suites no other beats on both joined as the frontier.
 */
export function ParetoChart(props: { points: ParetoPoint[] }) {
  return (
    <OverviewCard
      title={t('factoryInsights.pareto')}
      description={t('factoryInsights.paretoDescription')}
      class="lg:col-span-2"
    >
      <Show
        when={props.points.length > 1}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noPareto')}
            hint={t('factoryInsights.noParetoHint')}
          />
        }
      >
        <span class="text-[11px] text-ink-subtle">
          {t('factoryInsights.paretoScore')}
        </span>
        <InsightPlot
          label={t('factoryInsights.paretoChart')}
          height={260}
          options={(plot, width) => paretoPlot(plot, width, props.points)}
        />
        <p class="mt-2 flex items-center gap-4 text-[11px] text-ink-subtle">
          <span class="flex items-center gap-1.5">
            <span class="size-2.5 rounded-full bg-success" />
            {t('factoryInsights.onFrontier')}
          </span>
          <span class="flex items-center gap-1.5">
            <span class="size-2.5 rounded-full bg-ink-extra-muted" />
            {t('factoryInsights.offFrontier')}
          </span>
          <span class="ml-auto">{t('factoryInsights.paretoCost')}</span>
        </p>
      </Show>
    </OverviewCard>
  );
}

/** A CI job as the flaky table lists it, quarantined or not. */
type CiRow = {
  key: string;
  job: string;
  repository: string;
  finished?: number;
  redRate?: number;
  flaky: number;
  pullRequestsFailed?: number;
  quarantined?: { since: string; taskId?: string };
};

function ciRows(ci: QualityInsights['ci']): CiRow[] {
  const quarantined = new Map(
    ci.quarantined.map((row) => [`${row.repository}#${row.job}`, row])
  );
  const rows: CiRow[] = ci.jobs
    .filter(
      (job) =>
        job.red > 0 || quarantined.has(`${job.repository ?? ''}#${job.name}`)
    )
    .map((job) => {
      const key = `${job.repository ?? ''}#${job.name}`;
      const held = quarantined.get(key);
      quarantined.delete(key);
      return {
        key,
        job: job.name,
        repository: job.repository ?? '',
        finished: job.finished,
        redRate: job.redRate,
        flaky: job.causes.flaky,
        pullRequestsFailed: job.pullRequestsFailed,
        quarantined: held
          ? { since: held.quarantinedAt, taskId: held.taskId ?? undefined }
          : undefined,
      };
    });
  for (const [key, held] of quarantined) {
    rows.push({
      key,
      job: held.job,
      repository: held.repository,
      flaky: held.passedRetries,
      quarantined: {
        since: held.quarantinedAt,
        taskId: held.taskId ?? undefined,
      },
    });
  }
  return rows.sort((a, b) => (b.redRate ?? 1) - (a.redRate ?? 1));
}

/**
 * The CI jobs that went red in the period, worst first: how often, how
 * many of those were flaky, the pull requests they held, and whether the
 * factory quarantined them, with the task that tracks it.
 */
export function FlakyCiTable(props: {
  ci: QualityInsights['ci'];
  /**
   * Quarantines the job, or releases it; the table reads its rows again
   * once quality is read again. None leaves the jobs as they are.
   */
  onQuarantine?: (
    job: { repository: string; job: string },
    quarantine: boolean
  ) => Promise<void>;
}) {
  const rows = () => ciRows(props.ci);
  // The row whose job is being quarantined or released, by its key.
  const [changing, setChanging] = createSignal<string>();
  const change = async (row: CiRow, quarantine: boolean) => {
    if (!props.onQuarantine || changing()) return;
    setChanging(row.key);
    await props.onQuarantine(
      { repository: row.repository, job: row.job },
      quarantine
    );
    setChanging(undefined);
  };
  const ChangeButton = (button: { row: CiRow; quarantine: boolean }) => (
    <Show when={props.onQuarantine && button.row.repository}>
      <Button
        type="button"
        variant="ghost"
        size="xs"
        loading={changing() === button.row.key}
        disabled={changing() !== undefined}
        onClick={() =>
          fireAndForget(change(button.row, button.quarantine), 'factory-runs')
        }
      >
        {button.quarantine
          ? t('factoryInsights.ciQuarantine')
          : t('factoryInsights.ciRelease')}
      </Button>
    </Show>
  );
  return (
    <OverviewCard
      title={t('factoryInsights.flakyCi')}
      description={t('factoryInsights.flakyCiDescription')}
    >
      <Show
        when={rows().length > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noRedJobs')}
            hint={t('factoryInsights.noRedJobsHint')}
          />
        }
      >
        <table class="w-full table-fixed text-left text-xs">
          <colgroup>
            <col />
            <col class="w-24" />
            <col class="w-12" />
            <col class="w-26" />
          </colgroup>
          <thead>
            <tr class="text-[11px] text-ink-subtle">
              <th class="pb-2 font-medium">{t('factoryInsights.ciJob')}</th>
              <th class="pb-2 font-medium">{t('factoryInsights.ciRed')}</th>
              <th class="pb-2 text-right font-medium">
                {t('factoryInsights.ciFlaky')}
              </th>
              <th class="pb-2 text-right font-medium">
                {t('factoryInsights.ciStatus')}
              </th>
            </tr>
          </thead>
          <tbody class="divide-y divide-edge-muted">
            <For each={rows()}>
              {(row) => (
                <tr>
                  <td class="py-2.5 pr-3">
                    <span class="block truncate font-medium text-ink">
                      {row.job}
                    </span>
                    <span class="block truncate text-[11px] text-ink-subtle">
                      {repositoryName(row.repository)}
                    </span>
                  </td>
                  <td class="py-2.5 pr-3">
                    <Show
                      when={row.redRate !== undefined}
                      fallback={<span class="text-ink-subtle">·</span>}
                    >
                      <span class="flex items-center gap-2">
                        <span class="h-1.5 w-12 overflow-hidden rounded-full bg-edge-muted">
                          <span
                            class="block h-full rounded-full bg-failure"
                            style={{
                              width: `${Math.round((row.redRate ?? 0) * 100)}%`,
                            }}
                          />
                        </span>
                        <span class="tabular-nums text-ink">
                          {percent((row.redRate ?? 0) * 100)}
                        </span>
                      </span>
                    </Show>
                  </td>
                  <td class="py-2.5 text-right tabular-nums text-ink">
                    {row.flaky}
                  </td>
                  <td class="py-2.5 pl-3 text-right">
                    <Show
                      when={row.quarantined}
                      fallback={
                        <span class="inline-flex flex-col items-end gap-0.5">
                          <Badge variant="outline" size="xs">
                            {t('factoryInsights.ciActive')}
                          </Badge>
                          <ChangeButton row={row} quarantine />
                        </span>
                      }
                    >
                      {(held) => (
                        <span class="inline-flex flex-col items-end gap-0.5">
                          <Badge variant="violet" size="xs">
                            {t('factoryInsights.ciQuarantined')}
                          </Badge>
                          <ChangeButton row={row} quarantine={false} />
                          <Show when={held().taskId}>
                            {(taskId) => (
                              <a
                                class="text-[11px] text-ink-muted underline-offset-2 hover:text-ink hover:underline"
                                href={buildSimpleEntityUrl({
                                  type: 'task',
                                  id: taskId(),
                                })}
                              >
                                {t('factoryInsights.ciOpenTask')}
                              </a>
                            )}
                          </Show>
                        </span>
                      )}
                    </Show>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
        <p class="mt-3 text-[11px] text-ink-subtle">
          {t('factoryInsights.ciRetries', {
            passed: props.ci.retries.passed,
            failed: props.ci.retries.failed,
            refused: props.ci.retries.refused,
          })}
        </p>
      </Show>
    </OverviewCard>
  );
}

/**
 * Where Runs stop, compact: the Steps where Runs failed or went round
 * again, worst first, each as a bar of its Runs with the two counts.
 */
export function TroubleCard(props: { entries: EntryInsight[] }) {
  const top = () => props.entries.slice(0, 5);
  const most = () => Math.max(1, ...top().map((entry) => entry.troubledRuns));
  return (
    <OverviewCard
      title={t('factoryInsights.troubleCompact')}
      description={t('factoryInsights.troubleCompactDescription')}
    >
      <Show
        when={top().length > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noTrouble')}
            hint={t('factoryInsights.noRunsHint')}
          />
        }
      >
        <ul class="flex flex-col gap-3.5">
          <For each={top()}>
            {(entry) => (
              <li class="flex flex-col gap-1.5">
                <div class="flex items-baseline justify-between gap-2 text-xs">
                  <span class="font-medium text-ink">
                    {flowWordLabel(entry.entry)}
                  </span>
                  <span class="flex gap-2 tabular-nums text-ink-muted">
                    <Show when={entry.failedRuns > 0}>
                      <span class="text-failure-ink">
                        {t('factoryInsights.troubleFailed', {
                          count: entry.failedRuns,
                        })}
                      </span>
                    </Show>
                    <Show when={entry.revisitedRuns > 0}>
                      <span>
                        {t('factoryInsights.troubleAgain', {
                          count: entry.revisitedRuns,
                        })}
                      </span>
                    </Show>
                  </span>
                </div>
                <span class="flex h-1.5 w-full gap-0.5 overflow-hidden rounded-full bg-edge-muted">
                  <span
                    class="h-full rounded-full bg-failure"
                    style={{ width: `${(entry.failedRuns / most()) * 100}%` }}
                  />
                  <span
                    class="h-full rounded-full bg-warning"
                    style={{
                      width: `${(Math.max(0, entry.troubledRuns - entry.failedRuns) / most()) * 100}%`,
                    }}
                  />
                </span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </OverviewCard>
  );
}
