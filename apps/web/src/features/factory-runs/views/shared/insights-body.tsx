import { ListSkeleton } from '@app/components/view-shell/ListSkeleton';
import { EventOutcomesChart } from '@app/features/scheduling/components/event-outcomes-chart';
import {
  InsightSection,
  Metric,
  RankedList,
} from '@app/features/scheduling/components/insights-panel';
import { formatLocale, t } from '@app/lib/i18n';
import { Button } from '@ui';
import { Index, type JSX, Match, Show, Switch } from 'solid-js';
import { EntryList } from '../../components/entry-list';
import {
  INSIGHT_OUTCOME_COLORS,
  insightOutcomeLabel,
} from '../../components/insight-outcomes';
import {
  formatInsightDay,
  RunsByDayChart,
} from '../../components/runs-by-day-chart';
import { TroubleList } from '../../components/trouble-list';
import { UsageSummary } from '../../components/usage-summary';
import type { RunsSource } from '../../context/runs-context';
import { flowWordLabel } from '../../core/flow-words';
import {
  durationIn,
  INSIGHT_OUTCOMES,
  type RunInsights,
  runCount,
  successRate,
  troubledEntries,
} from '../../core/insights';

/** A duration as a figure and its unit, in the interface's language. */
function durationFigure(seconds: number | undefined) {
  if (seconds === undefined) return { value: undefined, unit: undefined };
  const { value, unit } = durationIn(seconds);
  const rounded = Math.round(value * 10) / 10;
  const parts = new Intl.NumberFormat(formatLocale(), {
    style: 'unit',
    unit,
    unitDisplay: 'short',
  }).formatToParts(rounded);
  return {
    value: rounded,
    unit: parts.find((part) => part.type === 'unit')?.value,
  };
}

/** Placeholders where the first section's figures land. */
function InsightsSkeleton() {
  return (
    <ListSkeleton.Root label={t('factoryInsights.loading')}>
      <div class="flex flex-col gap-5 border-t border-edge-muted py-5">
        <ListSkeleton.Bar class="h-4 w-24" />
        <div class="flex items-center gap-8">
          <ListSkeleton.Bar class="size-48 rounded-full" />
          <div class="flex flex-1 flex-col gap-3">
            <Index each={[0, 1, 2]}>
              {() => <ListSkeleton.Bar class="w-full max-w-64" />}
            </Index>
          </div>
        </div>
        <div class="grid grid-cols-2 gap-5">
          <Index each={[0, 1, 2, 3]}>
            {() => <ListSkeleton.Bar class="h-8 w-24" />}
          </Index>
        </div>
      </div>
    </ListSkeleton.Root>
  );
}

/** How the period's Runs went, once read. */
function InsightsSections(props: {
  insights: RunInsights;
  onOpenRun: (runId: string) => void;
}) {
  const totals = () => props.insights.totals;
  const rate = () => successRate(totals());
  const median = () =>
    durationFigure(props.insights.runDuration?.medianSeconds);
  const slowest = () => durationFigure(props.insights.runDuration?.p90Seconds);

  return (
    <div class="grid min-w-0 grid-cols-1 items-start gap-5 @min-[1000px]/insights:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
      <div class="@container/insight-main flex min-w-0 flex-col gap-5">
        <InsightSection title={t('factoryInsights.runs')}>
          <EventOutcomesChart
            rows={INSIGHT_OUTCOMES.map((outcome) => ({
              key: outcome,
              label: insightOutcomeLabel(outcome),
              count: totals()[outcome],
            }))}
            colors={INSIGHT_OUTCOME_COLORS}
            totalLabel={t('factoryInsights.totalRuns')}
            chartLabel={t('factoryInsights.outcomesChart')}
            listLabel={t('factoryInsights.outcomes')}
            emptyText={t('factoryInsights.noRunsInPeriod')}
            hintText={t('factoryInsights.oneStatePerRun')}
            showCountsLabel={t('factoryInsights.showCounts')}
            showPercentagesLabel={t('factoryInsights.showPercentages')}
            locale={formatLocale()}
            describe={(row, count, share) =>
              t('factoryInsights.outcomeShare', {
                outcome: row.label,
                count,
                share,
              })
            }
          />
          <div class="grid grid-cols-1 divide-y divide-edge-muted @sm/insight-main:grid-cols-2 @3xl/insight-main:grid-cols-4 @3xl/insight-main:divide-x @3xl/insight-main:divide-y-0">
            <Metric
              label={t('factoryInsights.totalRuns')}
              definition={t('factoryInsights.totalRunsDefinition')}
              value={runCount(totals())}
            />
            <Metric
              label={t('factoryInsights.successRate')}
              definition={t('factoryInsights.successRateDefinition')}
              value={
                rate() === undefined
                  ? undefined
                  : Math.round((rate() ?? 0) * 100)
              }
              unit="%"
            />
            <Metric
              label={t('factoryInsights.medianRunTime')}
              definition={t('factoryInsights.medianRunTimeDefinition')}
              value={median().value}
              unit={median().unit}
            />
            <Metric
              label={t('factoryInsights.slowRunTime')}
              definition={t('factoryInsights.slowRunTimeDefinition')}
              value={slowest().value}
              unit={slowest().unit}
              lowerIsBetter
            />
          </div>
        </InsightSection>
        <Show when={runCount(totals()) === 0}>
          <p class="rounded-lg border border-dashed border-edge-muted px-5 py-4 text-sm text-ink-muted">
            {t('factoryInsights.empty')}
          </p>
        </Show>
        <RunsByDayChart days={props.insights.days} />
        <EntryList entries={props.insights.entries} />
      </div>
      <div class="@container/insight-side flex min-w-0 flex-col gap-5">
        <RankedList
          title={t('factoryInsights.runsByFlow')}
          description={t('factoryInsights.runsByFlowDescription')}
          rows={props.insights.flows.map((flow) => ({
            id: flow.flow ?? '',
            name: flow.flow
              ? flowWordLabel(flow.flow)
              : t('factoryInsights.unrecordedFlow'),
            count: runCount(flow.counts),
          }))}
          empty={t('factoryInsights.noRunsInPeriod')}
        />
        <TroubleList
          entries={troubledEntries(props.insights.entries)}
          onOpenRun={props.onOpenRun}
        />
        <UsageSummary
          usage={props.insights.usage}
          onOpenRun={props.onOpenRun}
        />
      </div>
      <p class="col-span-full text-xs leading-relaxed text-ink-muted">
        {t('factoryInsights.footnote')}
      </p>
    </div>
  );
}

/**
 * The insights page below its header: the period it covers, then its
 * sections, a skeleton while the first read loads, or what went wrong.
 * `toolbar` sits above the period, where a view without a list header puts
 * its filters.
 */
export function InsightsBody(props: {
  source: RunsSource<RunInsights> & { updating: () => boolean };
  onOpenRun: (runId: string) => void;
  toolbar?: JSX.Element;
}) {
  const insights = () => props.source.data();

  return (
    <div
      class="@container/insights flex w-full min-w-0 flex-col gap-5"
      aria-busy={props.source.state() === 'pending' || props.source.updating()}
    >
      {props.toolbar}
      <Show when={insights()}>
        {(read) => (
          <p class="text-xs text-ink-muted">
            {t('factoryInsights.periodLine', {
              from: formatInsightDay(read().from),
              to: formatInsightDay(read().to),
              timeZone: read().timeZone.replaceAll('_', ' '),
            })}
          </p>
        )}
      </Show>
      <Switch>
        <Match when={props.source.state() === 'pending'}>
          <InsightsSkeleton />
        </Match>
        <Match when={props.source.state() === 'error'}>
          <div
            role="alert"
            class="flex flex-wrap items-center justify-between gap-4 rounded-xl border border-edge-muted p-5"
          >
            <p class="text-sm">{t('factoryInsights.loadFailed')}</p>
            <Button variant="secondary" onClick={props.source.retry}>
              {t('common.retry')}
            </Button>
          </div>
        </Match>
        <Match when={insights()}>
          {(read) => (
            <div classList={{ 'opacity-50': props.source.updating() }}>
              <Show when={read().capped}>
                <p class="mb-5 text-xs text-ink-muted">
                  {t('factoryInsights.capped', { count: read().runCap })}
                </p>
              </Show>
              <InsightsSections insights={read()} onOpenRun={props.onOpenRun} />
            </div>
          )}
        </Match>
      </Switch>
    </div>
  );
}
