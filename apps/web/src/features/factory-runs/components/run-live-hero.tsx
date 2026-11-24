import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { t } from '@app/lib/i18n';
import ArrowSquareOutIcon from '@icon/system/regular/arrow-square-out.svg';
import ClockIcon from '@icon/system/regular/clock.svg';
import GitMergeIcon from '@icon/system/regular/git-merge.svg';
import GitPullRequestIcon from '@icon/system/regular/git-pull-request.svg';
import TerminalIcon from '@icon/system/regular/terminal-window.svg';
import { Button, cn } from '@ui';
import { createSignal, For, type JSX, onCleanup, Show } from 'solid-js';
import { Dynamic } from 'solid-js/web';
import { repositoryName } from '../core/board';
import { flowWordLabel } from '../core/flow-words';
import {
  elapsedMs,
  flowStages,
  formatElapsed,
  mergedAt,
  type PipelineStage,
  runPipeline,
  workingSteps,
} from '../core/live';
import {
  type Run,
  type RunStep,
  runTitle,
  steerableSession,
} from '../core/run';
import {
  ActivityTail,
  ReviewerPills,
  RoleAvatar,
  RuntimeChips,
  reviewerName,
  StagePipeline,
  stageIcon,
  stepStatus,
} from './live-pipeline';
import { runOutcomeLabel } from './run-status';

/** One agent working now: who, on what, where, for how long. */
function AgentNow(props: {
  step: RunStep;
  now: number;
  panel: boolean;
  onWatch?: (step: RunStep) => void;
}) {
  const status = () => stepStatus(props.step);
  const waiting = () => status() === 'waiting';
  const ended = () => status() === 'done' || status() === 'failed';
  const name = () =>
    props.panel
      ? reviewerName(props.step.role)
      : props.step.role
        ? flowWordLabel(props.step.role)
        : t('factoryDashboard.theFactory');
  return (
    <li
      class={cn(
        'factory-live-enter flex min-w-0 flex-col gap-3 rounded-xl border p-4',
        waiting() && 'border-warning/40 bg-warning-bg/40',
        status() === 'working' && 'border-accent/35 bg-accent-bg/40',
        ended() && 'border-edge-muted bg-surface-1'
      )}
    >
      <div class="flex items-center gap-3">
        <RoleAvatar role={props.step.role} status={status()} size="lg" />
        <div class="flex min-w-0 flex-1 flex-col">
          <span class="truncate text-[14px] leading-5 font-semibold text-ink">
            {name()}
          </span>
          <span class="truncate text-xs text-ink-muted">
            {t('factoryDashboard.onStep', {
              step: flowWordLabel(props.step.entry),
            })}
          </span>
        </div>
        <div class="flex shrink-0 flex-col items-end">
          <span class="text-[11px] text-ink-subtle">
            {waiting()
              ? t('factoryDashboard.waitingFor')
              : ended()
                ? flowWordLabel(props.step.end?.outcome ?? '')
                : t('factoryDashboard.stepFor')}
          </span>
          <span
            class={cn(
              'text-lg leading-6 font-semibold tabular-nums',
              ended() ? 'text-ink-muted' : 'text-ink'
            )}
          >
            {formatElapsed(
              elapsedMs(
                waiting()
                  ? (props.step.gate?.openedAt ?? props.step.startedAt)
                  : props.step.startedAt,
                waiting() ? undefined : props.step.end?.endedAt,
                props.now
              )
            )}
          </span>
        </div>
      </div>
      <Show when={status() === 'working'}>
        <ActivityTail step={props.step} limit={props.panel ? 3 : 4} />
      </Show>
      <Show when={waiting() && props.step.gate}>
        {(gate) => (
          <p class="line-clamp-2 text-[12.5px] text-ink-muted">
            {gate().question}
          </p>
        )}
      </Show>
      <div class="flex flex-wrap items-center gap-2">
        <Show when={props.step.agent}>
          <RuntimeChips step={props.step} />
        </Show>
        <span class="flex-1" />
        <Show when={props.onWatch && steerableSession(props.step)}>
          <Button
            size="sm"
            variant={props.panel ? 'secondary' : 'primary'}
            onClick={() => props.onWatch?.(props.step)}
          >
            <TerminalIcon class="size-3.5" />
            {t('factoryDashboard.watchLive')}
          </Button>
        </Show>
      </div>
    </li>
  );
}

/**
 * How the Run ended, first thing on its page: merged with its pull
 * request, or where it stopped and why.
 */
function OutcomeCard(props: {
  run: Run;
  end: NonNullable<Run['end']>;
  stages: PipelineStage[];
}) {
  const merged = () =>
    props.end.outcome === 'success' || props.end.outcome === 'flawed';
  const cancelled = () =>
    props.end.outcome === 'cancelled' || props.end.outcome === 'manual';
  const stoppedAt = () =>
    props.stages.find(
      (stage) => stage.state === 'failed' || stage.state === 'stopped'
    );
  const pr = () => props.run.pullRequestUrl?.split('/').at(-1);
  // A merged Run counts to its merge, not to the Steps after it.
  const endedAt = () =>
    (merged() ? mergedAt(props.run) : undefined) ?? props.end.endedAt;
  const Icon = () =>
    merged() ? GitMergeIcon : stageIcon(stoppedAt()?.entry ?? '');
  return (
    <div
      class={cn(
        'factory-live-enter flex min-w-0 items-center gap-3 rounded-xl border py-2.5 pr-2.5 pl-3 sm:max-w-md',
        merged() && 'border-success/35 bg-success-bg/60',
        !merged() && cancelled() && 'border-edge-muted bg-surface-1',
        !merged() && !cancelled() && 'border-failure/35 bg-failure-bg/60'
      )}
    >
      <span
        class={cn(
          'flex size-9 shrink-0 items-center justify-center rounded-full',
          merged() && 'bg-success text-page',
          !merged() && cancelled() && 'bg-surface-3 text-ink-muted',
          !merged() && !cancelled() && 'bg-failure text-page'
        )}
      >
        <Dynamic component={Icon()} class="size-4.5" />
      </span>
      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
        <span class="truncate text-[14px] leading-5 font-semibold text-ink">
          {merged()
            ? pr()
              ? t('factoryDashboard.mergedPr', { number: pr() ?? '' })
              : runOutcomeLabel(props.end.outcome)
            : stoppedAt()
              ? t('factoryDashboard.stoppedAt', {
                  outcome: runOutcomeLabel(props.end.outcome),
                  step: flowWordLabel(stoppedAt()?.entry ?? ''),
                })
              : runOutcomeLabel(props.end.outcome)}
        </span>
        <span class="text-xs text-ink-muted">
          {t('factoryDashboard.endedSummary', {
            time: formatElapsed(elapsedMs(props.run.createdAt, endedAt())),
            ago: formatCompactRelativeTimestamp(endedAt()),
          })}
        </span>
      </div>
      <Show when={props.run.pullRequestUrl}>
        {(url) => (
          <a
            href={url()}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={t('factoryDashboard.openPr')}
            title={t('factoryDashboard.openPr')}
            class="ml-auto flex size-8 shrink-0 items-center justify-center rounded-full border border-edge-muted bg-surface text-ink-muted hover:bg-hover hover:text-ink"
          >
            <ArrowSquareOutIcon class="size-4" />
          </a>
        )}
      </Show>
    </div>
  );
}

/**
 * The top of a Run's page: where it is, its stages large with the working
 * one lit, and the agents working on it now, each a click from its live
 * session.
 */
export function RunLiveHero(props: {
  run: Run;
  entries?: readonly string[];
  onWatch?: (step: RunStep) => void;
  /** The owner's controls: Cancel run, or Retry run once it ended so. */
  actions?: JSX.Element;
}) {
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(clock));
  const stages = () =>
    runPipeline(
      props.run,
      flowStages(props.run.flow, props.entries, props.run.steps)
    );
  const working = () => {
    const steps = workingSteps(props.run);
    // A review panel shows every reviewer of its round, done ones too.
    const panel = stages().find((stage) => stage.state === 'current')?.latest;
    if (panel && panel.length > 1 && steps.length > 0) return panel;
    if (steps.length > 0) return steps;
    const last = props.run.steps.at(-1);
    return !props.run.end && last?.gate && !last.gate.answer ? [last] : [];
  };
  // A panel's reviewers that finished shrink to chips beside the heading.
  const finished = () =>
    working().length > 1
      ? working().filter((step) => step.end && !step.gate)
      : [];
  const active = () => working().filter((step) => !finished().includes(step));
  const state = (): 'working' | 'waiting' | 'merged' | 'ended' => {
    if (props.run.end) {
      return props.run.end.outcome === 'success' ? 'merged' : 'ended';
    }
    const last = props.run.steps.at(-1);
    return last?.gate && !last.gate.answer && last.end ? 'waiting' : 'working';
  };

  return (
    <section
      class="flex flex-col gap-4 pb-6"
      aria-label={t('factoryDashboard.pipeline')}
    >
      <div class="flex flex-wrap items-start gap-x-6 gap-y-3">
        <h1
          class="line-clamp-2 min-w-0 flex-1 basis-80 pt-0.5 text-[22px] leading-7 font-semibold tracking-tight text-balance text-ink"
          title={runTitle(props.run)}
        >
          {runTitle(props.run)}
        </h1>
        <Show when={props.run.end}>
          {(end) => (
            <OutcomeCard run={props.run} end={end()} stages={stages()} />
          )}
        </Show>
        <div class="flex shrink-0 items-center gap-2 empty:hidden">
          {props.actions}
        </div>
      </div>
      <div class="-mt-2 flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-ink-muted">
        <span
          class={cn(
            'inline-flex h-6 items-center gap-1.5 rounded-full px-2.5 font-medium',
            state() === 'working' && 'bg-accent-bg text-accent',
            state() === 'waiting' && 'bg-warning-bg text-warning',
            state() === 'merged' && 'bg-success-bg text-success',
            state() === 'ended' && 'bg-failure-bg text-failure'
          )}
        >
          <span
            class={cn(
              'relative size-1.5 rounded-full bg-current',
              state() === 'waiting' && 'factory-live-breathe'
            )}
          >
            <Show when={state() === 'working'}>
              <span class="factory-live-ripple" />
            </Show>
          </span>
          {state() === 'working'
            ? t('factoryDashboard.stageCurrent')
            : state() === 'waiting'
              ? t('factoryDashboard.waitingOnYou')
              : state() === 'merged' && props.run.pullRequestUrl
                ? t('factoryDashboard.merged')
                : runOutcomeLabel(props.run.end?.outcome ?? 'success')}
        </span>
        <Show when={props.run.repository}>
          {(repo) => <span class="text-ink">{repositoryName(repo().url)}</span>}
        </Show>
        <Show when={props.run.flow}>
          {(flow) => <span>{flowWordLabel(flow())}</span>}
        </Show>
        <span class="flex items-center gap-1 tabular-nums">
          <ClockIcon class="size-3.5" />
          {props.run.end
            ? t('factoryDashboard.tookTotal')
            : t('factoryDashboard.runFor')}{' '}
          <span class="font-medium text-ink">
            {formatElapsed(
              elapsedMs(props.run.createdAt, props.run.end?.endedAt, now())
            )}
          </span>
        </span>
        <Show when={props.run.pullRequestUrl}>
          {(url) => (
            <a
              href={url()}
              target="_blank"
              rel="noopener noreferrer"
              class="flex items-center gap-1 rounded-md px-1 text-ink-muted hover:bg-hover hover:text-ink"
            >
              <GitPullRequestIcon class="size-3.5" />
              <span class="tabular-nums">#{url().split('/').at(-1)}</span>
            </a>
          )}
        </Show>
      </div>
      <div class="rounded-2xl border border-edge-muted bg-surface-1 px-4 pt-5 pb-4">
        <StagePipeline stages={stages()} now={now()} />
      </div>
      <Show when={working().length > 0}>
        <div class="flex flex-col gap-2.5">
          <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
            <h2 class="flex items-center gap-2 text-[13px] font-semibold text-ink">
              <span class="size-2 rounded-full bg-accent" />
              {t('factoryDashboard.now')}
            </h2>
            <Show when={finished().length > 0}>
              <ReviewerPills steps={finished()} />
            </Show>
          </div>
          <ul
            class={cn(
              'grid gap-2.5',
              active().length > 2 && 'grid-cols-1 md:grid-cols-3',
              active().length === 2 && 'grid-cols-1 md:grid-cols-2',
              active().length === 1 && 'grid-cols-1 md:max-w-2xl'
            )}
          >
            <For each={active()}>
              {(step) => (
                <AgentNow
                  step={step}
                  now={now()}
                  panel={working().length > 1}
                  onWatch={props.onWatch}
                />
              )}
            </For>
          </ul>
        </div>
      </Show>
    </section>
  );
}
