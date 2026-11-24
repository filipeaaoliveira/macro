import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { cn } from '@ui';
import { type Accessor, createMemo, For, Show } from 'solid-js';
import {
  FlakyCiTable,
  KpiStrip,
  OverviewCard,
  OverviewEmpty,
  ParetoChart,
  RoleCards,
  RunsOverTime,
  StepTimeChart,
  TroubleCard,
} from '../../components/insights-overview';
import { CalibrationBadge } from '../../components/scorer-calibration';
import { useInsightsContext } from '../../context/insights-context';
import type { RunsSource } from '../../context/runs-context';
import {
  type EvaluationInsights,
  type ExperimentInsight,
  type ScorerInsight,
  suiteStandings,
} from '../../core/evaluation';
import { flowWordLabel } from '../../core/flow-words';
import {
  insightKpis,
  paretoPoints,
  previousParams,
  roleInsights,
  stepTimes,
} from '../../core/insight-overview';
import {
  type InsightsParams,
  type RunInsights,
  troubledEntries,
} from '../../core/insights';
import type { DeliveryInsight } from '../../core/quality';

const percent = (share: number) => `${Math.round(share * 100)}%`;

/** A share as a thin bar in its tone. */
function ShareBar(props: { share: number; tone: 'success' | 'accent' }) {
  return (
    <span class="h-1.5 w-full overflow-hidden rounded-full bg-edge-muted">
      <span
        class={cn(
          'block h-full rounded-full',
          props.tone === 'success' ? 'bg-success' : 'bg-accent'
        )}
        style={{ width: `${Math.round(props.share * 100)}%` }}
      />
    </span>
  );
}

/** A card's place while what fills it loads. */
function Pending(props: { class?: string }) {
  return (
    <div
      aria-hidden="true"
      class={cn(
        'min-h-64 animate-pulse rounded-2xl border border-edge-muted bg-surface-1',
        props.class
      )}
    />
  );
}

/** Each Scorer: how many of its judgments passed, and whether it is calibrated. */
function ScorersCard(props: { scorers: ScorerInsight[] }) {
  return (
    <OverviewCard
      title={t('factoryInsights.scorerPassRates')}
      description={t('factoryInsights.scorersSummary')}
    >
      <Show
        when={props.scorers.length > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryEvaluation.noScorers')}
            hint={t('factoryInsights.noScorersHint')}
          />
        }
      >
        <ul class="flex flex-col gap-4">
          <For each={props.scorers}>
            {(scorer) => (
              <li class="flex flex-col gap-1.5">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">
                    {flowWordLabel(scorer.name)}
                  </span>
                  <CalibrationBadge calibration={scorer.calibration} />
                </div>
                <div class="flex items-center gap-3">
                  <ShareBar
                    share={
                      scorer.judged > 0 ? scorer.passed / scorer.judged : 0
                    }
                    tone="success"
                  />
                  <span class="w-10 shrink-0 text-right text-xs font-semibold tabular-nums text-ink">
                    {scorer.judged > 0
                      ? percent(scorer.passed / scorer.judged)
                      : '0%'}
                  </span>
                </div>
                <span class="text-[11px] text-ink-subtle">
                  {t('factoryInsights.scorerLine', {
                    passed: scorer.passed,
                    judged: scorer.judged,
                    labelled: scorer.calibration.labelled,
                    needed: scorer.calibration.needed,
                  })}
                </span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </OverviewCard>
  );
}

/** Each Experiment's arms side by side: Runs, how many succeeded. */
function ExperimentsCard(props: { experiments: ExperimentInsight[] }) {
  return (
    <OverviewCard
      title={t('factoryInsights.experimentArms')}
      description={t('factoryInsights.experimentsSummary')}
    >
      <Show
        when={props.experiments.length > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryEvaluation.noExperiments')}
            hint={t('factoryInsights.noExperimentsHint')}
          />
        }
      >
        <ul class="flex flex-col gap-5">
          <For each={props.experiments}>
            {(experiment) => (
              <li class="flex flex-col gap-2.5">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 truncate text-[13px] font-semibold text-ink">
                    {flowWordLabel(experiment.name)}
                  </span>
                  <Show when={experiment.running}>
                    <span class="flex items-center gap-1 text-[11px] text-accent">
                      <span class="size-1.5 rounded-full bg-accent" />
                      {t('factoryInsights.experimentRunning')}
                    </span>
                  </Show>
                </div>
                <For each={experiment.arms}>
                  {(arm) => (
                    <div class="flex flex-col gap-1">
                      <div class="flex items-baseline justify-between gap-2 text-xs">
                        <span class="truncate text-ink">
                          {flowWordLabel(arm.name)}
                        </span>
                        <span class="shrink-0 tabular-nums text-ink-muted">
                          {t('factoryInsights.armLine', {
                            succeeded: arm.succeeded,
                            runs: arm.runs,
                          })}
                        </span>
                      </div>
                      <ShareBar
                        share={arm.ended > 0 ? arm.succeeded / arm.ended : 0}
                        tone="accent"
                      />
                    </div>
                  )}
                </For>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </OverviewCard>
  );
}

/**
 * How much people stepped in: of the Runs that ended, those that merged on
 * their own, those with a Gate a person answered, a Step they steered, and
 * those left to a person.
 */
function PeopleCard(props: { delivery: DeliveryInsight }) {
  const rows = () => [
    {
      label: t('factoryInsights.peopleAutonomous'),
      count: props.delivery.autonomous,
      tone: 'success' as const,
    },
    {
      label: t('factoryInsights.peopleAnswered'),
      count: props.delivery.personAnswered,
      tone: 'accent' as const,
    },
    {
      label: t('factoryInsights.peopleSteered'),
      count: props.delivery.steered,
      tone: 'accent' as const,
    },
    {
      label: t('factoryInsights.peopleManual'),
      count: props.delivery.manual,
      tone: 'accent' as const,
    },
  ];
  return (
    <OverviewCard
      title={t('factoryInsights.people')}
      description={t('factoryInsights.peopleDescription', {
        count: props.delivery.endedRuns,
      })}
    >
      <Show
        when={props.delivery.endedRuns > 0}
        fallback={
          <OverviewEmpty
            title={t('factoryInsights.noEndedRuns')}
            hint={t('factoryInsights.noRunsHint')}
          />
        }
      >
        <ul class="flex flex-col gap-4">
          <For each={rows()}>
            {(row) => (
              <li class="flex flex-col gap-1.5">
                <div class="flex items-baseline justify-between gap-2 text-xs">
                  <span class="text-ink">{row.label}</span>
                  <span class="tabular-nums text-ink-muted">
                    <span class="font-semibold text-ink">{row.count}</span>
                    {' · '}
                    {percent(row.count / props.delivery.endedRuns)}
                  </span>
                </div>
                <ShareBar
                  share={row.count / props.delivery.endedRuns}
                  tone={row.tone}
                />
              </li>
            )}
          </For>
        </ul>
      </Show>
    </OverviewCard>
  );
}

/**
 * The top of the insights page: its headline figures against the period
 * before, the Runs over time, where their time goes, each Role, flaky CI,
 * Scorers, Experiments and the cost/quality frontier.
 */
export function InsightsOverviewPanel(props: {
  params: Accessor<InsightsParams>;
  source: RunsSource<RunInsights>;
}) {
  const context = useInsightsContext();
  const before = createMemo(() => previousParams(props.params()));
  const previous = context.insights(before);
  const quality = context.quality(props.params);
  const previousQuality = context.quality(before);
  const evaluation = context.evaluation(props.params);
  const points = (read: EvaluationInsights) =>
    paretoPoints(read.suites, suiteStandings(read.suites));

  return (
    <Show when={props.source.data()}>
      {(insights) => (
        <div class="flex min-w-0 flex-col gap-4 pb-8">
          <KpiStrip
            retried={insights().totals.retried}
            kpis={insightKpis({
              insights: insights(),
              previous: previous.data(),
              quality: quality.data(),
              previousQuality: previousQuality.data(),
            })}
          />
          <div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
            <RunsOverTime days={insights().days} totals={insights().totals} />
            <Show
              when={evaluation.state() !== 'pending'}
              fallback={<Pending />}
            >
              <ScorersCard scorers={evaluation.data()?.scorers ?? []} />
            </Show>
            <StepTimeChart steps={stepTimes(insights().entries)} />
            <div class="flex min-w-0 flex-col gap-4">
              <TroubleCard entries={troubledEntries(insights().entries)} />
              <Show when={quality.data()}>
                {(read) => (
                  <FlakyCiTable
                    ci={read().ci}
                    onQuarantine={async (job, quarantine) => {
                      const result = await context.setCiQuarantine(
                        job,
                        quarantine
                      );
                      if (!result.ok) {
                        toast.failure(
                          result.message ??
                            t('factoryInsights.ciQuarantineFailed')
                        );
                      }
                    }}
                  />
                )}
              </Show>
            </div>
            <RoleCards
              roles={roleInsights(
                insights().entries,
                quality.data()?.review.byReviewer
              )}
            />
            <Show
              when={evaluation.state() !== 'pending'}
              fallback={<Pending class="lg:col-span-2" />}
            >
              <ParetoChart
                points={evaluation.data() ? points(evaluation.data()!) : []}
              />
            </Show>
            <div class="flex min-w-0 flex-col gap-4">
              <Show when={quality.data()}>
                {(read) => <PeopleCard delivery={read().delivery} />}
              </Show>
              <Show when={evaluation.state() !== 'pending'}>
                <ExperimentsCard
                  experiments={evaluation.data()?.experiments ?? []}
                />
              </Show>
            </div>
          </div>
          <p class="text-[11px] text-ink-subtle">
            {t('factoryInsights.overviewFootnote')}
          </p>
        </div>
      )}
    </Show>
  );
}
