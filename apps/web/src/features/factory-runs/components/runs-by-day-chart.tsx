import {
  integerTick,
  TIP,
} from '@app/features/scheduling/components/insight-chart-options';
import {
  InsightPlot,
  type InsightPlotModule,
} from '@app/features/scheduling/components/insight-plot';
import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { formatLocale, t } from '@app/lib/i18n';
import type { PlotOptions } from '@observablehq/plot';
import { createSignal, For } from 'solid-js';
import {
  INSIGHT_OUTCOMES,
  type InsightDay,
  type InsightOutcome,
} from '../core/insights';
import { insightOutcomeColor, insightOutcomeLabel } from './insight-outcomes';

/** A day of the period, as an axis or a table names it. */
export function formatInsightDay(date: string, long = false): string {
  return new Intl.DateTimeFormat(formatLocale(), {
    timeZone: 'UTC',
    month: 'short',
    day: 'numeric',
    ...(long ? { weekday: 'short' } : {}),
  }).format(new Date(`${date}T12:00:00Z`));
}

/** The Runs created each day, stacked by how they went. */
function runsByDayPlot(
  Plot: InsightPlotModule,
  width: number,
  days: InsightDay[],
  outcomes: InsightOutcome[]
): PlotOptions {
  const data = days.flatMap((day) =>
    outcomes.map((outcome) => ({
      date: new Date(`${day.date}T00:00:00Z`),
      count: day.counts[outcome],
      outcome: insightOutcomeLabel(outcome),
      tip: [
        formatInsightDay(day.date, true),
        ...outcomes.map(
          (shown) => `${insightOutcomeLabel(shown)}: ${day.counts[shown]}`
        ),
      ].join('\n'),
    }))
  );
  const most = Math.max(
    4,
    ...days.map((day) =>
      outcomes.reduce((sum, outcome) => sum + day.counts[outcome], 0)
    )
  );
  const labelEvery = Math.max(
    1,
    Math.ceil(days.length / Math.max(2, Math.floor(width / 80)))
  );
  return {
    width,
    height: 240,
    marginLeft: 40,
    marginRight: 18,
    marginBottom: 32,
    x: {
      type: 'band',
      label: null,
      tickSize: 0,
      tickFormat: (date: Date) =>
        formatInsightDay(date.toISOString().slice(0, 10)),
      // A label every few bars, as many as the width has room for.
      ticks: days
        .filter((_, index) => index % labelEvery === 0)
        .map((day) => new Date(`${day.date}T00:00:00Z`)),
    },
    y: {
      label: null,
      grid: true,
      domain: [0, most],
      ticks: 4,
      tickFormat: integerTick,
    },
    color: {
      domain: outcomes.map(insightOutcomeLabel),
      range: outcomes.map(insightOutcomeColor),
      legend: false,
    },
    marks: [
      Plot.barY(data, {
        x: 'date',
        y: 'count',
        fill: 'outcome',
        inset: days.length > 45 ? 0.5 : 2,
        title: 'tip',
        tip: TIP,
      }),
      Plot.ruleY([0], { stroke: 'var(--color-edge)' }),
    ],
  };
}

/** The period's Runs by day, with a legend that hides and shows states. */
export function RunsByDayChart(props: { days: InsightDay[] }) {
  const [visible, setVisible] = createSignal<InsightOutcome[]>([
    ...INSIGHT_OUTCOMES,
  ]);
  const plotted = () =>
    INSIGHT_OUTCOMES.filter((outcome) => visible().includes(outcome));

  return (
    <InsightSection
      title={t('factoryInsights.runsByDay')}
      actions={
        <div
          class="flex flex-wrap gap-1.5"
          role="group"
          aria-label={t('factoryInsights.visibleStates')}
        >
          <For each={INSIGHT_OUTCOMES}>
            {(outcome) => (
              <button
                type="button"
                aria-pressed={visible().includes(outcome)}
                onClick={() =>
                  setVisible((current) =>
                    current.includes(outcome)
                      ? current.filter((key) => key !== outcome)
                      : [...current, outcome]
                  )
                }
                class="inline-flex items-center gap-1.5 rounded-md border border-edge-muted bg-panel px-2 py-1 text-xs text-ink-muted hover:bg-hover focus-visible:outline-2 focus-visible:outline-ink"
                classList={{ 'opacity-40': !visible().includes(outcome) }}
              >
                <span
                  class="size-2 rounded-full"
                  style={{ background: insightOutcomeColor(outcome) }}
                />
                {insightOutcomeLabel(outcome)}
              </button>
            )}
          </For>
        </div>
      }
    >
      <div class="px-4 pb-4 pt-3 sm:px-6">
        <InsightPlot
          label={t('factoryInsights.runsByDayChart')}
          height={240}
          options={(plot, width) =>
            runsByDayPlot(plot, width, props.days, plotted())
          }
        />
        <details class="mt-4 text-xs text-ink-muted">
          <summary class="w-fit rounded py-1 focus-visible:outline-2 focus-visible:outline-ink">
            {t('factoryInsights.viewDailyData')}
          </summary>
          <div class="mt-2 max-h-60 overflow-auto rounded-lg border border-edge-muted">
            <table class="w-full text-left tabular-nums">
              <caption class="sr-only">
                {t('factoryInsights.runsByDay')}
              </caption>
              <thead class="bg-surface-2">
                <tr>
                  <th class="p-3">{t('factoryInsights.date')}</th>
                  <For each={INSIGHT_OUTCOMES}>
                    {(outcome) => (
                      <th class="p-3 font-medium">
                        {insightOutcomeLabel(outcome)}
                      </th>
                    )}
                  </For>
                </tr>
              </thead>
              <tbody>
                <For each={props.days}>
                  {(day) => (
                    <tr class="border-t border-edge-muted">
                      <th class="whitespace-nowrap p-3 font-normal">
                        {formatInsightDay(day.date)}
                      </th>
                      <For each={INSIGHT_OUTCOMES}>
                        {(outcome) => (
                          <td class="p-3">{day.counts[outcome]}</td>
                        )}
                      </For>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        </details>
      </div>
    </InsightSection>
  );
}
