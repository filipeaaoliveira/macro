import {
  integerTick,
  TIP,
} from '@app/features/scheduling/components/insight-chart-options';
import {
  InsightPlot,
  type InsightPlotModule,
} from '@app/features/scheduling/components/insight-plot';
import {
  DailyDataTable,
  DayExplorer,
  SeriesToggles,
  toggleSeries,
} from '@app/features/scheduling/components/insight-trend-parts';
import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { formatLocale, t } from '@app/lib/i18n';
import type { PlotOptions } from '@observablehq/plot';
import { createSignal, For } from 'solid-js';
import {
  INSIGHT_OUTCOMES,
  type InsightDay,
  type InsightOutcome,
} from '../core/insights';
import {
  INSIGHT_OUTCOME_COLORS,
  insightOutcomeLabel,
} from './insight-outcomes';

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
      range: outcomes.map((outcome) => INSIGHT_OUTCOME_COLORS[outcome]),
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
  const series = () =>
    INSIGHT_OUTCOMES.map((outcome) => ({
      key: outcome,
      label: insightOutcomeLabel(outcome),
      color: INSIGHT_OUTCOME_COLORS[outcome],
    }));
  const [visible, setVisible] = createSignal<InsightOutcome[]>([
    ...INSIGHT_OUTCOMES,
  ]);
  const plotted = () =>
    INSIGHT_OUTCOMES.filter((outcome) => visible().includes(outcome));

  return (
    <InsightSection
      title={t('factoryInsights.runsByDay')}
      actions={
        <SeriesToggles
          label={t('factoryInsights.visibleStates')}
          series={series()}
          visible={visible()}
          onToggle={(key) =>
            setVisible((current) => toggleSeries(current, key))
          }
        />
      }
    >
      <div class="px-4 pb-4 pt-3 sm:px-6">
        <DayExplorer
          days={props.days}
          label={t('factoryInsights.runsByDayExplore')}
          status={(day) => (
            <>
              <span class="font-medium text-ink">
                {formatInsightDay(day.date, true)}
              </span>
              <For each={plotted()}>
                {(outcome) => (
                  <span>
                    {insightOutcomeLabel(outcome)}: {day.counts[outcome]}
                  </span>
                )}
              </For>
            </>
          )}
        >
          <InsightPlot
            label={t('factoryInsights.runsByDayChart')}
            height={240}
            options={(plot, width) =>
              runsByDayPlot(plot, width, props.days, plotted())
            }
          />
        </DayExplorer>
        <DailyDataTable
          summary={t('factoryInsights.viewDailyData')}
          caption={t('factoryInsights.runsByDay')}
          dateHeading={t('factoryInsights.date')}
          days={props.days}
          date={(day) => formatInsightDay(day.date)}
          columns={series()}
          value={(day, key) => day.counts[key]}
        />
      </div>
    </InsightSection>
  );
}
