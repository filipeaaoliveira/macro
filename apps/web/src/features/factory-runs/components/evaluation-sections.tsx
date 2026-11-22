import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { Button } from '@ui';
import { createSignal, createUniqueId, For, type JSX, Show } from 'solid-js';
import { useInsightsContext } from '../context/insights-context';
import type {
  AgentRuleInsight,
  ArmInsight,
  BenchmarkSummary,
  EvaluationInsights,
  ExperimentInsight,
  ImprovementInsight,
  ImprovementState,
  NamerStanding,
  ScorerInsight,
  SelfImprovementInsight,
  SuiteInsight,
} from '../core/evaluation';
import { flowWordLabel } from '../core/flow-words';
import { tokenTotal } from '../core/insights';
import { formatRunDate, formatShare, formatTokens } from './insight-format';
import {
  INSIGHT_OUTCOME_COLORS,
  insightOutcomeLabel,
} from './insight-outcomes';

function Figure(props: { label: string; children: JSX.Element }) {
  return (
    <div class="flex min-w-0 flex-col gap-0.5">
      <dt class="text-xs text-ink-muted">{props.label}</dt>
      <dd class="text-sm font-medium tabular-nums">{props.children}</dd>
    </div>
  );
}

/** A score from 0 to 1, as two decimals, or a dash when there is none. */
function formatScore(score: number | null | undefined): string {
  return score === null || score === undefined ? '–' : score.toFixed(2);
}

/** `writer: backup · planner: primary`, or the definition's own. */
function agentsLine(agents: Record<string, string>): string {
  const entries = Object.entries(agents);
  if (entries.length === 0) return t('factoryEvaluation.definitionAgents');
  return entries
    .map(([role, agent]) => t('factoryEvaluation.roleAgent', { role, agent }))
    .join(' · ');
}

function EmptyRow(props: { children: JSX.Element }) {
  return (
    <li class="py-8 text-center text-sm text-ink-muted">{props.children}</li>
  );
}

function ScorerRow(props: { scorer: ScorerInsight }) {
  const sample = () => {
    const percent = props.scorer.samplePercent;
    if (percent === null || percent === undefined) {
      return t('factoryEvaluation.retired');
    }
    const share = formatShare(percent / 100);
    return props.scorer.flows.length === 0
      ? t('factoryEvaluation.sampled', { share })
      : t('factoryEvaluation.sampledFlows', {
          share,
          flows: props.scorer.flows.map(flowWordLabel).join(', '),
        });
  };

  return (
    <li class="flex flex-col gap-3 px-4 py-4 sm:px-5">
      <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 class="text-sm font-medium">{props.scorer.name}</h3>
        <span class="text-xs text-ink-muted">{sample()}</span>
      </div>
      <Show when={props.scorer.instructions}>
        {(instructions) => (
          <p class="text-xs text-ink-muted line-clamp-2">{instructions()}</p>
        )}
      </Show>
      <dl class="grid grid-cols-3 gap-x-4 gap-y-3">
        <Figure label={t('factoryEvaluation.judged')}>
          {props.scorer.judged}
        </Figure>
        <Figure label={t('factoryEvaluation.meanScore')}>
          {formatScore(props.scorer.meanScore)}
        </Figure>
        <Figure label={t('factoryEvaluation.passed')}>
          {props.scorer.judged > 0
            ? formatShare(props.scorer.passed / props.scorer.judged)
            : '–'}
        </Figure>
      </dl>
    </li>
  );
}

function ArmRow(props: { arm: ArmInsight }) {
  return (
    <tr class="border-t border-edge-muted">
      <th scope="row" class="py-2 pr-3 text-left font-medium">
        <span class="block">{props.arm.name}</span>
        <span class="block text-xs font-normal text-ink-muted">
          {agentsLine(props.arm.agents)}
        </span>
        <Show when={props.arm.fellBack > 0}>
          <span class="block text-xs font-normal text-ink-muted">
            {t('factoryEvaluation.fellBack', { count: props.arm.fellBack })}
          </span>
        </Show>
      </th>
      <td class="py-2 pr-3 tabular-nums">{props.arm.runs}</td>
      <td class="py-2 pr-3 tabular-nums">
        {props.arm.ended > 0
          ? formatShare(props.arm.succeeded / props.arm.ended)
          : '–'}
      </td>
      <td class="py-2 text-xs text-ink-muted">
        <For each={props.arm.scores} fallback="–">
          {(score, index) => (
            <>
              <Show when={index() > 0}>{' · '}</Show>
              <span>{score.scorer}</span>{' '}
              <span class="tabular-nums text-ink">
                {formatScore(score.meanScore)}
              </span>
            </>
          )}
        </For>
      </td>
    </tr>
  );
}

function ExperimentRow(props: { experiment: ExperimentInsight }) {
  return (
    <li class="flex flex-col gap-2 px-4 py-4 sm:px-5">
      <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 class="text-sm font-medium">{props.experiment.name}</h3>
        <span class="text-xs text-ink-muted">
          {props.experiment.flow
            ? flowWordLabel(props.experiment.flow)
            : t('factoryEvaluation.ended')}
        </span>
      </div>
      <div class="overflow-x-auto">
        <table class="w-full min-w-[28rem] text-sm">
          <thead class="text-xs text-ink-muted">
            <tr>
              <th scope="col" class="pb-1 pr-3 text-left font-normal">
                {t('factoryEvaluation.arm')}
              </th>
              <th scope="col" class="pb-1 pr-3 text-left font-normal">
                {t('factoryEvaluation.armRuns')}
              </th>
              <th scope="col" class="pb-1 pr-3 text-left font-normal">
                {t('factoryEvaluation.armSucceeded')}
              </th>
              <th scope="col" class="pb-1 text-left font-normal">
                {t('factoryEvaluation.meanScore')}
              </th>
            </tr>
          </thead>
          <tbody>
            <For each={props.experiment.arms}>
              {(arm) => <ArmRow arm={arm} />}
            </For>
          </tbody>
        </table>
      </div>
    </li>
  );
}

function AgentRuleRow(props: { rule: AgentRuleInsight }) {
  return (
    <tr class="border-t border-edge-muted">
      <th scope="row" class="py-2 pr-3 text-left font-medium">
        <span class="block">{props.rule.rule}</span>
        <span class="block text-xs font-normal text-ink-muted">
          {t('factoryEvaluation.agentRuleChose', {
            role: props.rule.role,
            agent: props.rule.agent,
          })}
        </span>
        <Show when={props.rule.fellBack > 0}>
          <span class="block text-xs font-normal text-ink-muted">
            {t('factoryEvaluation.fellBack', { count: props.rule.fellBack })}
          </span>
        </Show>
      </th>
      <td class="py-2 pr-3 tabular-nums">{props.rule.runs}</td>
      <td class="py-2 tabular-nums">
        {props.rule.ended > 0
          ? formatShare(props.rule.succeeded / props.rule.ended)
          : '–'}
      </td>
    </tr>
  );
}

/** The Agent Rules that chose Agents for the period's Runs, side by side. */
function AgentRulesTable(props: { rules: AgentRuleInsight[] }) {
  return (
    <div class="overflow-x-auto px-4 py-4 sm:px-5">
      <table data-agent-rules class="w-full min-w-[20rem] text-sm">
        <thead class="text-xs text-ink-muted">
          <tr>
            <th scope="col" class="pb-1 pr-3 text-left font-normal">
              {t('factoryEvaluation.agentRule')}
            </th>
            <th scope="col" class="pb-1 pr-3 text-left font-normal">
              {t('factoryEvaluation.armRuns')}
            </th>
            <th scope="col" class="pb-1 text-left font-normal">
              {t('factoryEvaluation.armSucceeded')}
            </th>
          </tr>
        </thead>
        <tbody>
          <For each={props.rules}>{(rule) => <AgentRuleRow rule={rule} />}</For>
        </tbody>
      </table>
    </div>
  );
}

function SuiteRow(props: {
  suite: SuiteInsight;
  onOpenRun: (runId: string) => void;
}) {
  return (
    <li class="flex flex-col gap-3 px-4 py-4 sm:px-5">
      <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 class="text-sm font-medium">
          {props.suite.benchmark}
          <span class="ml-2 text-xs font-normal text-ink-muted">
            {formatRunDate(props.suite.createdAt)}
          </span>
        </h3>
        <span class="text-xs text-ink-muted">
          {agentsLine(props.suite.agents)}
        </span>
      </div>
      <dl class="grid grid-cols-3 gap-x-4 gap-y-3">
        <Figure label={t('factoryEvaluation.score')}>
          {formatScore(props.suite.meanScore)}
        </Figure>
        <Figure label={t('factoryEvaluation.tokens')}>
          {formatTokens(tokenTotal(props.suite.tokens))}
        </Figure>
        <Figure label={t('factoryEvaluation.trial')}>
          {t('factoryEvaluation.suiteScored', {
            scored: props.suite.scored,
            count: props.suite.trials.length,
          })}
          <Show when={props.suite.infrastructure > 0}>
            <span class="block text-xs font-normal text-ink-muted">
              {t('factoryEvaluation.suiteInfrastructure', {
                count: props.suite.infrastructure,
              })}
            </span>
          </Show>
        </Figure>
      </dl>
      <ul aria-label={t('factoryEvaluation.trial')} class="-mx-2 flex flex-col">
        <For each={props.suite.trials}>
          {(trial) => (
            <li>
              <button
                type="button"
                class="flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm not-touch:hover:bg-hover focus-visible:outline-2 focus-visible:outline-ink touch:min-h-11"
                onClick={() => props.onOpenRun(trial.runId)}
              >
                <span
                  class="size-2 shrink-0 rounded-full"
                  style={{
                    background:
                      INSIGHT_OUTCOME_COLORS[trial.runOutcome ?? 'running'],
                  }}
                />
                <span class="sr-only">
                  {insightOutcomeLabel(trial.runOutcome ?? 'running')}
                </span>
                <span class="min-w-0 flex-1 truncate">{trial.task}</span>
                <For each={trial.scores}>
                  {(score) => (
                    <span class="shrink-0 text-xs text-ink-muted tabular-nums">
                      {score.scorer} {formatScore(score.score)}
                    </span>
                  )}
                </For>
                <span class="shrink-0 text-xs text-ink-muted tabular-nums">
                  {trial.tokens ? formatTokens(tokenTotal(trial.tokens)) : '–'}
                </span>
                <span class="shrink-0 text-xs font-medium tabular-nums">
                  {formatScore(trial.score)}
                </span>
              </button>
            </li>
          )}
        </For>
      </ul>
    </li>
  );
}

function BenchmarkRow(props: { benchmark: BenchmarkSummary }) {
  return (
    <li class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 px-4 py-3 sm:px-5">
      <h3 class="text-sm font-medium">{props.benchmark.name}</h3>
      <span class="text-xs text-ink-muted">
        {t('factoryEvaluation.tasksCount', {
          count: props.benchmark.tasks.length,
        })}
        <Show when={props.benchmark.scorers.length > 0}>
          {' · '}
          {t('factoryEvaluation.judgedBy', {
            scorers: props.benchmark.scorers.join(', '),
          })}
        </Show>
      </span>
    </li>
  );
}

const IMPROVEMENT_STATE_LABELS: Record<ImprovementState, () => string> = {
  starting: () => t('factoryEvaluation.improvementStarting'),
  open: () => t('factoryEvaluation.improvementOpen'),
  merged: () => t('factoryEvaluation.improvementMerged'),
  closed: () => t('factoryEvaluation.improvementClosed'),
  refused: () => t('factoryEvaluation.improvementRefused'),
};

/** The flawed Runs' source reads as words; a Scorer by its name. */
function sourceLabel(source: string): string {
  return source === 'flawed' ? t('factoryEvaluation.flawedSource') : source;
}

/**
 * Undo dismissal, for a rejected self-improvement run whose failures the
 * person may count again: confirmed in place, as Cancel run is, so no
 * dialog stacks on a phone's sheet.
 */
function UndoDismissal(props: { improvementId: string }) {
  const context = useInsightsContext();
  const [confirming, setConfirming] = createSignal(false);
  const [undoing, setUndoing] = createSignal(false);
  const messageId = createUniqueId();
  let undoButton: HTMLButtonElement | undefined;
  let keepButton: HTMLButtonElement | undefined;

  const askToUndo = () => {
    setConfirming(true);
    queueMicrotask(() => keepButton?.focus());
  };

  const keepDismissed = () => {
    setConfirming(false);
    queueMicrotask(() => undoButton?.focus());
  };

  const undo = async () => {
    if (undoing()) return;
    setUndoing(true);
    const result = await context.undoDismissal(props.improvementId);
    setUndoing(false);
    if (result.ok) return;
    // The confirmation closes; focus goes back to Undo dismissal, if it
    // is still offered, rather than to the page.
    setConfirming(false);
    queueMicrotask(() => undoButton?.focus());
    if (result.reason === 'conflict') {
      toast.alert(t('factoryEvaluation.undoDismissalConflict'));
      return;
    }
    if (result.reason === 'forbidden') {
      toast.failure(t('factoryEvaluation.undoDismissalForbidden'));
      return;
    }
    toast.failure(t('factoryEvaluation.undoDismissalFailed'));
  };

  return (
    <Show
      when={confirming()}
      fallback={
        <div>
          <Button
            ref={undoButton}
            type="button"
            variant="ghost"
            onClick={askToUndo}
          >
            {t('factoryEvaluation.undoDismissal')}
          </Button>
        </div>
      }
    >
      <div class="flex w-full flex-wrap items-center justify-end gap-2">
        <p id={messageId} class="mr-auto min-w-0 flex-1 basis-60 text-sm">
          <span class="font-medium">
            {t('factoryEvaluation.undoDismissalTitle')}
          </span>{' '}
          <span class="text-ink-muted">
            {t('factoryEvaluation.undoDismissalBody')}
          </span>
        </p>
        <Button
          ref={keepButton}
          type="button"
          variant="ghost"
          aria-describedby={messageId}
          disabled={undoing()}
          onClick={keepDismissed}
        >
          {t('factoryEvaluation.keepDismissed')}
        </Button>
        <Button
          type="button"
          variant="primary"
          aria-describedby={messageId}
          loading={undoing()}
          onClick={() => fireAndForget(undo(), 'factory-runs')}
        >
          {t('factoryEvaluation.undoDismissal')}
        </Button>
      </div>
    </Show>
  );
}

function LastImprovement(props: {
  last: ImprovementInsight;
  onOpenRun: (runId: string) => void;
}) {
  return (
    <div class="flex flex-col gap-1.5 border-t border-edge-muted pt-3">
      <h3 class="text-xs text-ink-muted">
        {t('factoryEvaluation.lastImprovement')}
      </h3>
      <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        <span class="font-medium">
          {IMPROVEMENT_STATE_LABELS[props.last.state]()}
        </span>
        <span class="text-xs text-ink-muted">
          {formatRunDate(props.last.createdAt)}
          {' · '}
          {t('factoryEvaluation.improvementFailures', {
            count: props.last.failures,
          })}
        </span>
        <Show when={props.last.runId}>
          {(runId) => (
            <button
              type="button"
              class="rounded-md px-2 py-1 text-xs font-medium not-touch:hover:bg-hover focus-visible:outline-2 focus-visible:outline-ink touch:min-h-11"
              onClick={() => props.onOpenRun(runId())}
            >
              {t('factoryEvaluation.openImprovement')}
            </button>
          )}
        </Show>
      </div>
      <Show when={props.last.refusal}>
        {(refusal) => (
          <p class="text-xs text-ink-muted line-clamp-2">{refusal()}</p>
        )}
      </Show>
      <Show when={props.last.dismissed > 0}>
        <p class="text-xs text-ink-muted">
          {t('factoryEvaluation.improvementDismissed', {
            count: props.last.dismissed,
          })}
        </p>
      </Show>
      <Show when={props.last.dismissalUndoneAt}>
        {(undoneAt) => (
          <p class="text-xs text-ink-muted">
            {t('factoryEvaluation.dismissalUndone', {
              time: formatRunDate(undoneAt()),
            })}
          </p>
        )}
      </Show>
      <Show when={props.last.canUndoDismissal}>
        <UndoDismissal improvementId={props.last.id} />
      </Show>
    </div>
  );
}

/**
 * Why no self-improvement run starts as the admin who named the factory
 * definition; none while they are still an admin of the team.
 */
function namerProblem(namer: NamerStanding): string | undefined {
  switch (namer) {
    case 'admin':
      return undefined;
    case 'none':
      return t('factoryEvaluation.improvementNoOwner');
    case 'left_team':
      return t('factoryEvaluation.improvementNamerLeft');
    case 'not_admin':
      return t('factoryEvaluation.improvementNamerNotAdmin');
  }
}

/**
 * The team's self-improvement: the failures waiting for a run against its
 * factory definition, what starts one, and the latest one.
 */
function SelfImprovementBody(props: {
  improvement: SelfImprovementInsight;
  onOpenRun: (runId: string) => void;
}) {
  return (
    <div class="flex flex-col gap-3 px-4 py-4 sm:px-5">
      <dl class="grid grid-cols-3 gap-x-4 gap-y-3">
        <Figure label={t('factoryEvaluation.pendingFailures')}>
          {props.improvement.pending}
        </Figure>
        <Figure label={t('factoryEvaluation.startsAt')}>
          {t('factoryEvaluation.startsAtValue', {
            count: props.improvement.failuresThreshold,
            days: props.improvement.oldestDays,
          })}
        </Figure>
        <Figure label={t('factoryEvaluation.oldestPending')}>
          {props.improvement.oldestPendingAt
            ? formatRunDate(props.improvement.oldestPendingAt)
            : '–'}
        </Figure>
      </dl>
      <Show when={props.improvement.pendingBySource.length > 0}>
        <p class="text-xs text-ink-muted">
          <For each={props.improvement.pendingBySource}>
            {(source, index) => (
              <>
                <Show when={index() > 0}>{' · '}</Show>
                <span>{sourceLabel(source.source)}</span>{' '}
                <span class="tabular-nums text-ink">{source.count}</span>
              </>
            )}
          </For>
        </p>
      </Show>
      <Show when={!props.improvement.enabled}>
        <p class="text-xs text-ink-muted">
          {t('factoryEvaluation.improvementOff')}
        </p>
      </Show>
      <Show
        when={
          props.improvement.enabled && namerProblem(props.improvement.namer)
        }
      >
        {(problem) => <p class="text-xs text-ink-muted">{problem()}</p>}
      </Show>
      <Show
        when={props.improvement.last}
        fallback={
          <p class="border-t border-edge-muted pt-3 text-xs text-ink-muted">
            {t('factoryEvaluation.noImprovement')}
          </p>
        }
      >
        {(last) => (
          <LastImprovement last={last()} onOpenRun={props.onOpenRun} />
        )}
      </Show>
    </div>
  );
}

/**
 * How the period's Runs were judged and compared: each Scorer's judgments,
 * each Experiment's arms side by side, the Agent Rules that chose Agents,
 * and the Benchmarks with the suites
 * replayed in the period, trial by trial.
 */
export function EvaluationSections(props: {
  evaluation: EvaluationInsights;
  onOpenRun: (runId: string) => void;
}) {
  return (
    <div class="flex min-w-0 flex-col gap-5">
      <InsightSection
        title={t('factoryEvaluation.scorers')}
        description={t('factoryEvaluation.scorersDescription')}
      >
        <ul class="flex flex-col divide-y divide-edge-muted">
          <For
            each={props.evaluation.scorers}
            fallback={<EmptyRow>{t('factoryEvaluation.noScorers')}</EmptyRow>}
          >
            {(scorer) => <ScorerRow scorer={scorer} />}
          </For>
        </ul>
      </InsightSection>
      <Show when={props.evaluation.selfImprovement}>
        {(improvement) => (
          <InsightSection
            title={t('factoryEvaluation.selfImprovement')}
            description={t('factoryEvaluation.selfImprovementDescription')}
          >
            <SelfImprovementBody
              improvement={improvement()}
              onOpenRun={props.onOpenRun}
            />
          </InsightSection>
        )}
      </Show>
      <InsightSection
        title={t('factoryEvaluation.experiments')}
        description={t('factoryEvaluation.experimentsDescription')}
      >
        <ul class="flex flex-col divide-y divide-edge-muted">
          <For
            each={props.evaluation.experiments}
            fallback={
              <EmptyRow>{t('factoryEvaluation.noExperiments')}</EmptyRow>
            }
          >
            {(experiment) => <ExperimentRow experiment={experiment} />}
          </For>
        </ul>
      </InsightSection>
      <Show when={(props.evaluation.agentRules ?? []).length > 0}>
        <InsightSection
          title={t('factoryEvaluation.agentRules')}
          description={t('factoryEvaluation.agentRulesDescription')}
        >
          <AgentRulesTable rules={props.evaluation.agentRules ?? []} />
        </InsightSection>
      </Show>
      <InsightSection
        title={t('factoryEvaluation.benchmarks')}
        description={t('factoryEvaluation.benchmarksDescription')}
      >
        <ul class="flex flex-col divide-y divide-edge-muted">
          <For
            each={props.evaluation.benchmarks}
            fallback={
              <EmptyRow>{t('factoryEvaluation.noBenchmarks')}</EmptyRow>
            }
          >
            {(benchmark) => <BenchmarkRow benchmark={benchmark} />}
          </For>
        </ul>
        <Show
          when={
            props.evaluation.benchmarks.length > 0 ||
            props.evaluation.suites.length > 0
          }
        >
          <ul class="flex flex-col divide-y divide-edge-muted border-t border-edge-muted">
            <For
              each={props.evaluation.suites}
              fallback={<EmptyRow>{t('factoryEvaluation.noSuites')}</EmptyRow>}
            >
              {(suite) => (
                <SuiteRow suite={suite} onOpenRun={props.onOpenRun} />
              )}
            </For>
          </ul>
        </Show>
      </InsightSection>
    </div>
  );
}
