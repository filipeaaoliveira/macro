import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import ArrowSquareOutIcon from '@icon/system/regular/arrow-square-out.svg';
import ChatIcon from '@icon/system/regular/chat-circle-text.svg';
import CheckCircleIcon from '@icon/system/regular/check-circle.svg';
import ClockIcon from '@icon/system/regular/clock.svg';
import FlaskIcon from '@icon/system/regular/flask.svg';
import GitMergeIcon from '@icon/system/regular/git-merge.svg';
import GitPullRequestIcon from '@icon/system/regular/git-pull-request.svg';
import HandIcon from '@icon/system/regular/hand.svg';
import ListNumbersIcon from '@icon/system/regular/list-numbers.svg';
import MinusCircleIcon from '@icon/system/regular/minus-circle.svg';
import UsersThreeIcon from '@icon/system/regular/users-three.svg';
import WarningCircleIcon from '@icon/system/regular/warning-circle.svg';
import XCircleIcon from '@icon/system/regular/x-circle.svg';
import { Button, cn } from '@ui';
import {
  type Component,
  createSignal,
  For,
  type JSX,
  Match,
  Show,
  Switch,
} from 'solid-js';
import { Dynamic } from 'solid-js/web';
import { formatTokens } from '../components/insight-format';
import {
  ActivityTail,
  ReviewerPills,
  RoleAvatar,
  StageTrack,
} from '../components/live-pipeline';
import { runOutcomeLabel } from '../components/run-status';
import type { RunsSource } from '../context/runs-context';
import { type BoardRun, repositoryName } from '../core/board';
import { flowWordLabel } from '../core/flow-words';
import {
  elapsedMs,
  flowStages,
  formatElapsed,
  type GateType,
  gateType,
  type LiveNumbers,
  runPipeline,
  workingSteps,
} from '../core/live';
import { type DeliveryInsight, mergedCost } from '../core/quality';
import {
  APPROVE_ANSWER,
  type Gate,
  isQuestion,
  isTampered,
  type Run,
  type RunStep,
  runTitle,
} from '../core/run';

type Icon = Component<JSX.SvgSVGAttributes<SVGSVGElement>>;

/** What the dashboard reads; each part loads on its own. */
export type DashboardData = {
  /** Runs working now: a Step runs, or the merge waits on a red base. */
  active: RunsSource<BoardRun[]>;
  /** Runs waiting on a person at a Gate. */
  waiting: RunsSource<BoardRun[]>;
  /** The newest Runs, ended ones among them. */
  recent: RunsSource<BoardRun[]>;
  /** The Runs holding a place in the person's Queue, and its size. */
  queue: RunsSource<{ held: Run[]; places: number }>;
  queueLimit: RunsSource<number | null>;
  numbers: () => LiveNumbers;
  /** How the last week's Runs delivered: autonomy and cost; none unread. */
  delivery?: () => DeliveryInsight | undefined;
};

/** A Run's title: its task's name, else its request's first line. */
function titleOf(listed: BoardRun): string {
  return listed.taskName ?? runTitle(listed.run);
}

function repoOf(run: Run): string | undefined {
  return run.repository ? repositoryName(run.repository.url) : undefined;
}

const GATE_ICONS: Record<GateType, Icon> = {
  plan: ListNumbersIcon,
  blockers: UsersThreeIcon,
  ci: FlaskIcon,
  merge: GitMergeIcon,
  question: ChatIcon,
  decision: HandIcon,
};

function gateTypeLabel(type: GateType): string {
  switch (type) {
    case 'plan':
      return t('factoryDashboard.gatePlan');
    case 'blockers':
      return t('factoryDashboard.gateBlockers');
    case 'ci':
      return t('factoryDashboard.gateCi');
    case 'merge':
      return t('factoryDashboard.gateMerge');
    case 'question':
      return t('factoryDashboard.gateQuestion');
    case 'decision':
      return t('factoryDashboard.gateDecision');
  }
}

/** The header strip: what runs, what waits, what merged, how it goes. */
function LiveStrip(props: {
  numbers: LiveNumbers;
  delivery?: DeliveryInsight;
}) {
  const cost = () => (props.delivery ? mergedCost(props.delivery) : undefined);
  const autonomy = () => props.delivery?.autonomyRate ?? undefined;
  const Cell = (cell: {
    label: string;
    value: string;
    hint?: string;
    tone?: 'accent' | 'warning' | 'success';
    live?: boolean;
  }) => (
    <div class="flex min-w-[150px] flex-1 flex-col gap-1 px-5 py-3 first:pl-0">
      <dt class="flex items-center gap-1.5 text-xs text-ink-muted">
        <Show when={cell.tone}>
          <span
            class={cn(
              'relative size-1.5 rounded-full',
              cell.tone === 'accent' && 'bg-accent text-accent',
              cell.tone === 'warning' && 'bg-warning text-warning',
              cell.tone === 'success' && 'bg-success'
            )}
          >
            <Show when={cell.live}>
              <span class="factory-live-ripple" />
            </Show>
          </span>
        </Show>
        {cell.label}
      </dt>
      <dd class="flex items-baseline gap-2">
        <span class="text-2xl leading-7 font-semibold tracking-tight tabular-nums text-ink">
          {cell.value}
        </span>
        <Show when={cell.hint}>
          <span class="truncate text-[11px] text-ink-subtle">{cell.hint}</span>
        </Show>
      </dd>
    </div>
  );

  return (
    <dl class="flex flex-wrap border-b border-edge-divider sm:divide-x sm:divide-edge-divider">
      <Cell
        label={t('factoryDashboard.running')}
        value={String(props.numbers.running)}
        tone="accent"
        live={props.numbers.running > 0}
      />
      <Cell
        label={t('factoryDashboard.waitingOnYou')}
        value={String(props.numbers.waiting)}
        tone="warning"
        live={props.numbers.waiting > 0}
      />
      <Cell
        label={t('factoryDashboard.mergedToday')}
        value={String(props.numbers.mergedToday)}
        tone="success"
        hint={
          props.numbers.medianToMerge === undefined
            ? undefined
            : t('factoryDashboard.medianToMerge', {
                time: formatElapsed(props.numbers.medianToMerge),
              })
        }
      />
      <Show when={props.numbers.successRate !== undefined}>
        <Cell
          label={t('factoryDashboard.successRate')}
          value={`${props.numbers.successRate ?? 0}%`}
          hint={t('factoryDashboard.ofEnded', { count: props.numbers.ended })}
        />
      </Show>
      <Show when={autonomy() !== undefined}>
        <Cell
          label={t('factoryDashboard.autonomy')}
          value={`${Math.round((autonomy() ?? 0) * 100)}%`}
          hint={t('factoryDashboard.lastWeek')}
        />
      </Show>
      <Show when={cost()}>
        {(value) => (
          <Cell
            label={t('factoryDashboard.costPerMerge')}
            value={
              value().kind === 'money'
                ? `$${(value() as { usd: number }).usd.toFixed(2)}`
                : formatTokens((value() as { tokens: number }).tokens)
            }
            hint={t('factoryDashboard.lastWeek')}
          />
        )}
      </Show>
    </dl>
  );
}

function SectionHeading(props: {
  title: string;
  count?: number;
  tone?: 'warning' | 'accent';
  children?: JSX.Element;
}) {
  return (
    <div class="flex items-center gap-2 pb-2.5">
      <h2 class="flex items-center gap-2 text-[13px] font-semibold text-ink">
        <Show when={props.tone}>
          <span
            class={cn(
              'size-2 rounded-full',
              props.tone === 'warning' && 'bg-warning factory-live-breathe',
              props.tone === 'accent' && 'bg-accent'
            )}
          />
        </Show>
        {props.title}
      </h2>
      <Show when={props.count !== undefined}>
        <span class="rounded-full bg-surface-2 px-1.5 text-[11px] leading-4.5 font-medium tabular-nums text-ink-muted">
          {props.count}
        </span>
      </Show>
      <span class="flex-1" />
      {props.children}
    </div>
  );
}

/** The open Gate of a Run waiting on a person, and its Step. */
function openGate(run: Run): { step: RunStep; gate: Gate } | undefined {
  const step = run.steps.at(-1);
  return step?.gate && !step.gate.answer
    ? { step, gate: step.gate }
    : undefined;
}

/**
 * A Gate waiting on a person: what kind, which Run, what it asks, and its
 * answers. An answer that needs words or an approval of Control Files
 * opens the Run instead.
 */
function GateItem(props: {
  listed: BoardRun;
  owned: boolean;
  age: (time: string) => string;
  onOpen: () => void;
  onAnswer: (step: RunStep, answer: string) => Promise<boolean>;
}) {
  const [answering, setAnswering] = createSignal<string>();
  const open = () => openGate(props.listed.run);
  const type = () => {
    const gate = open();
    return gate ? gateType(gate.step, gate.gate) : 'decision';
  };
  // An agent's question takes words, and approving a Tampered Run takes a
  // look at its files: both answer in the Run.
  const inline = () => {
    const gate = open();
    if (!gate || !props.owned || isQuestion(gate.gate)) return false;
    const files = props.listed.run.controlFiles;
    return !(
      files &&
      isTampered(files) &&
      gate.gate.answers.includes(APPROVE_ANSWER)
    );
  };
  const answer = async (step: RunStep, value: string) => {
    setAnswering(value);
    await props.onAnswer(step, value);
    setAnswering(undefined);
  };

  return (
    <Show when={open()}>
      {(gate) => (
        <li class="factory-live-enter flex flex-col gap-3 rounded-xl border border-warning/30 bg-warning-bg/50 p-3.5 sm:flex-row sm:items-center">
          <span class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-warning/15 text-warning">
            <Dynamic component={GATE_ICONS[type()]} class="size-4.5" />
          </span>
          <div class="flex min-w-0 flex-1 flex-col gap-0.5">
            <div class="flex items-center gap-2 text-[11.5px]">
              <span class="font-semibold text-warning-ink">
                {gateTypeLabel(type())}
              </span>
              <span class="text-ink-subtle">
                {t('factoryDashboard.askedAgo', {
                  time: props.age(gate().gate.openedAt),
                })}
              </span>
            </div>
            <button
              type="button"
              class="truncate text-left text-[13.5px] font-medium text-ink outline-none hover:underline focus-visible:underline"
              onClick={() => props.onOpen()}
            >
              {titleOf(props.listed)}
            </button>
            <p class="line-clamp-1 text-xs text-ink-muted">
              {gate().gate.question}
            </p>
          </div>
          <div class="flex shrink-0 items-center gap-1.5" role="group">
            <Show
              when={inline()}
              fallback={
                <Button
                  size="sm"
                  variant="primary"
                  onClick={() => props.onOpen()}
                >
                  {isQuestion(gate().gate)
                    ? t('factoryDashboard.answerInRun')
                    : t('factoryDashboard.reviewInRun')}
                </Button>
              }
            >
              <For each={gate().gate.answers.slice(1).reverse()}>
                {(value) => (
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={answering() !== undefined}
                    loading={answering() === value}
                    onClick={() =>
                      fireAndForget(answer(gate().step, value), 'factory-runs')
                    }
                  >
                    {flowWordLabel(value)}
                  </Button>
                )}
              </For>
              <Show when={gate().gate.answers[0]}>
                {(value) => (
                  <Button
                    size="sm"
                    variant="primary"
                    disabled={answering() !== undefined}
                    loading={answering() === value()}
                    onClick={() =>
                      fireAndForget(
                        answer(gate().step, value()),
                        'factory-runs'
                      )
                    }
                  >
                    {flowWordLabel(value())}
                  </Button>
                )}
              </Show>
            </Show>
          </div>
        </li>
      )}
    </Show>
  );
}

/**
 * One Run working now: its task and repository, who works on it now and
 * what they did last, for how long, and its stages as a track.
 */
function ActiveRunCard(props: {
  listed: BoardRun;
  entries?: (flow: string) => readonly string[] | undefined;
  now: number;
  onOpen: () => void;
}) {
  const run = () => props.listed.run;
  const stages = () =>
    runPipeline(
      run(),
      flowStages(
        run().flow,
        run().flow ? props.entries?.(run().flow ?? '') : undefined,
        run().steps
      )
    );
  const working = () => workingSteps(run());
  const lead = () => working()[0] ?? run().steps.at(-1);
  const reviewing = () => (working().length > 1 ? working() : undefined);
  const panel = () => {
    const current = stages().find((stage) => stage.state === 'current');
    return current && current.latest.length > 1 ? current.latest : undefined;
  };
  const held = () => props.listed.state === 'held';

  return (
    <li class="factory-live-enter">
      <div
        role="button"
        tabIndex={0}
        class="group flex cursor-pointer flex-col gap-3 rounded-xl border border-edge-muted bg-surface-1 px-4 pt-3.5 pb-3 outline-none transition-colors duration-150 not-touch:hover:border-edge focus-visible:border-accent"
        onClick={() => props.onOpen()}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            props.onOpen();
          }
        }}
      >
        <div class="flex flex-col gap-3 md:flex-row md:items-center md:gap-5">
          <div class="flex min-w-0 flex-1 flex-col gap-0.5">
            <span class="truncate text-[14px] leading-5 font-semibold text-ink">
              {titleOf(props.listed)}
            </span>
            <span class="flex min-w-0 items-center gap-1.5 truncate text-xs text-ink-subtle">
              <Show when={repoOf(run())}>
                {(repo) => <span class="truncate">{repo()}</span>}
              </Show>
              <span aria-hidden="true">·</span>
              <span class="flex shrink-0 items-center gap-1 tabular-nums">
                <ClockIcon class="size-3" />
                {formatElapsed(
                  elapsedMs(run().createdAt, undefined, props.now)
                )}
              </span>
              <Show when={run().pullRequestUrl}>
                {(url) => (
                  <>
                    <span aria-hidden="true">·</span>
                    <a
                      href={url()}
                      target="_blank"
                      rel="noopener noreferrer"
                      class="flex shrink-0 items-center gap-0.5 rounded hover:text-ink"
                      onClick={(event) => event.stopPropagation()}
                    >
                      <GitPullRequestIcon class="size-3" />
                      <span class="tabular-nums">
                        #{url().split('/').at(-1)}
                      </span>
                    </a>
                  </>
                )}
              </Show>
            </span>
          </div>
          <div class="flex min-w-0 items-center gap-2.5 md:w-[46%] md:max-w-[440px]">
            <Switch>
              <Match when={held()}>
                <span class="flex items-center gap-2 text-xs text-warning">
                  <WarningCircleIcon class="size-4" />
                  {t('factoryDashboard.heldByBase', {
                    branch: run().repository?.baseBranch ?? '',
                  })}
                </span>
              </Match>
              <Match when={reviewing() && (panel() ?? reviewing())}>
                {(steps) => (
                  <>
                    <RoleAvatar panel status="working" />
                    <div class="flex min-w-0 flex-1 flex-col gap-1">
                      <span class="truncate text-[12.5px] leading-4 font-medium text-ink">
                        {t('factoryDashboard.panelReviewing', {
                          count: steps().length,
                        })}
                      </span>
                      <ReviewerPills steps={steps()} />
                    </div>
                    <StepClock
                      from={steps()[0]?.startedAt ?? ''}
                      now={props.now}
                    />
                  </>
                )}
              </Match>
              <Match when={lead()}>
                {(step) => (
                  <>
                    <RoleAvatar
                      role={step().role}
                      status={step().end ? 'waiting' : 'working'}
                    />
                    <div class="flex min-w-0 flex-1 flex-col">
                      <span class="truncate text-[12.5px] leading-4 font-medium text-ink">
                        {step().role
                          ? flowWordLabel(step().role ?? '')
                          : t('factoryDashboard.theFactory')}
                        <span class="font-normal text-ink-subtle">
                          {' · '}
                          {flowWordLabel(step().entry)}
                        </span>
                      </span>
                      <Show
                        when={!step().end}
                        fallback={
                          <span class="truncate text-[12px] leading-4.5 text-ink-subtle">
                            {t('factoryDashboard.betweenSteps')}
                          </span>
                        }
                      >
                        <ActivityTail step={step()} compact />
                      </Show>
                    </div>
                    <StepClock from={step().startedAt} now={props.now} />
                  </>
                )}
              </Match>
            </Switch>
          </div>
        </div>
        <StageTrack stages={stages()} />
      </div>
    </li>
  );
}

/** How long the work now has gone on, ticking. */
function StepClock(props: { from: string; now: number }) {
  return (
    <span
      class="shrink-0 text-right text-[13px] font-medium tabular-nums text-ink"
      title={t('factoryDashboard.stepFor')}
    >
      {formatElapsed(elapsedMs(props.from, undefined, props.now))}
    </span>
  );
}

/**
 * The person's slots: how many of their Queue's places Runs hold, as dots,
 * each a Run waiting on them or still before its first Gate.
 */
function SlotsPanel(props: {
  places: number;
  limit: number | null | undefined;
}) {
  const slots = () =>
    Array.from(
      { length: Math.max(props.limit ?? props.places, props.places) },
      (_, i) => i
    );
  return (
    <section aria-label={t('factoryDashboard.slots')}>
      <SectionHeading title={t('factoryDashboard.slots')}>
        <span class="text-xs tabular-nums text-ink-muted">
          {props.limit
            ? t('factoryDashboard.slotsOf', {
                count: props.places,
                limit: props.limit,
              })
            : t('factoryDashboard.slotsUnlimited', { count: props.places })}
        </span>
      </SectionHeading>
      <div class="flex flex-wrap gap-1.5">
        <For each={slots()}>
          {(slot) => (
            <span
              class={cn(
                'h-2 min-w-6 flex-1 rounded-full transition-colors duration-300',
                slot < props.places ? 'bg-accent' : 'bg-edge-muted'
              )}
            />
          )}
        </For>
      </div>
      <p class="mt-2 text-[11.5px] leading-4 text-ink-subtle">
        {t('factoryDashboard.slotsHint')}
      </p>
    </section>
  );
}

function OutcomeIcon(props: { run: Run }) {
  const outcome = () => props.run.end?.outcome;
  return (
    <span
      class={cn(
        'flex size-7 shrink-0 items-center justify-center rounded-full',
        outcome() === 'success' && 'bg-success-bg text-success',
        (outcome() === 'failed' ||
          outcome() === 'blocked' ||
          outcome() === 'flawed') &&
          'bg-failure-bg text-failure',
        (outcome() === 'manual' ||
          outcome() === 'cancelled' ||
          outcome() === 'infrastructure') &&
          'bg-surface-2 text-ink-muted'
      )}
    >
      <Switch fallback={<MinusCircleIcon class="size-3.5" />}>
        <Match when={outcome() === 'success'}>
          <GitMergeIcon class="size-3.5" />
        </Match>
        <Match when={outcome() === 'manual'}>
          <HandIcon class="size-3.5" />
        </Match>
        <Match
          when={
            outcome() === 'failed' ||
            outcome() === 'blocked' ||
            outcome() === 'flawed'
          }
        >
          <XCircleIcon class="size-3.5" />
        </Match>
      </Switch>
    </span>
  );
}

/** Runs that ended lately: how, when, how long they took, their PR. */
function RecentPanel(props: {
  runs: BoardRun[];
  age: (time: string) => string;
  onOpen: (runId: string) => void;
}) {
  return (
    <section aria-label={t('factoryDashboard.recent')}>
      <SectionHeading title={t('factoryDashboard.recent')} />
      <Show
        when={props.runs.length > 0}
        fallback={
          <p class="rounded-lg border border-dashed border-edge-muted px-3 py-3 text-xs text-ink-subtle">
            {t('factoryDashboard.recentEmpty')}
          </p>
        }
      >
        <ul class="-mx-2 flex flex-col">
          <For each={props.runs}>
            {(listed) => (
              <li class="group relative flex items-center rounded-lg hover:bg-hover">
                <button
                  type="button"
                  class="flex min-w-0 flex-1 items-center gap-2.5 px-2 py-2 text-left outline-none focus-visible:outline-2 focus-visible:outline-accent"
                  onClick={() => props.onOpen(listed.run.id)}
                >
                  <OutcomeIcon run={listed.run} />
                  <span class="flex min-w-0 flex-1 flex-col">
                    <span class="truncate text-[12.5px] leading-4.5 font-medium text-ink">
                      {titleOf(listed)}
                    </span>
                    <span class="truncate text-[11px] leading-4 text-ink-subtle">
                      {[
                        listed.run.end
                          ? runOutcomeLabel(listed.run.end.outcome)
                          : undefined,
                        listed.run.end
                          ? props.age(listed.run.end.endedAt)
                          : undefined,
                        listed.run.end
                          ? formatElapsed(
                              elapsedMs(
                                listed.run.createdAt,
                                listed.run.end.endedAt
                              )
                            )
                          : undefined,
                      ]
                        .filter(Boolean)
                        .join(' · ')}
                    </span>
                  </span>
                </button>
                <Show when={listed.run.pullRequestUrl}>
                  {(url) => (
                    <a
                      href={url()}
                      target="_blank"
                      rel="noopener noreferrer"
                      aria-label={t('factoryDashboard.pullRequest')}
                      class="mr-1.5 flex h-6 shrink-0 items-center gap-1 rounded-md px-1.5 text-[11px] text-ink-subtle hover:bg-active hover:text-ink"
                    >
                      <span class="tabular-nums">
                        #{url().split('/').at(-1)}
                      </span>
                      <ArrowSquareOutIcon class="size-3" />
                    </a>
                  )}
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </section>
  );
}

/** A section while it loads: a few quiet bars in its place. */
function Skeleton(props: { rows: number; tall?: boolean }) {
  return (
    <div class="flex flex-col gap-2">
      <For each={Array.from({ length: props.rows })}>
        {() => (
          <div
            class={cn(
              'animate-pulse rounded-xl bg-surface-1',
              props.tall ? 'h-32' : 'h-10'
            )}
          />
        )}
      </For>
    </div>
  );
}

/**
 * The factory's live dashboard: numbers, what waits on the person, what
 * runs now and where, the Queue, and what ended lately.
 */
export function DashboardView(props: {
  data: DashboardData;
  viewerId: string | undefined;
  entries?: (flow: string) => readonly string[] | undefined;
  now: number;
  onOpenRun: (runId: string) => void;
  onAnswer: (input: {
    runId: string;
    step: RunStep;
    answer: string;
  }) => Promise<boolean>;
}) {
  const age = (time: string) => {
    props.now;
    return formatCompactRelativeTimestamp(time);
  };
  const waiting = () => props.data.waiting.data() ?? [];
  const active = () => props.data.active.data() ?? [];
  const ended = () =>
    (props.data.recent.data() ?? [])
      .filter((listed) => listed.run.end)
      .slice(0, 8);

  return (
    <div class="mx-auto flex w-full max-w-[1320px] flex-col gap-6 px-4 pb-10 sm:px-6">
      <LiveStrip
        numbers={props.data.numbers()}
        delivery={props.data.delivery?.()}
      />
      <div class="grid grid-cols-1 gap-8 lg:grid-cols-[minmax(0,1fr)_320px]">
        <div class="flex min-w-0 flex-col gap-7">
          <Show when={waiting().length > 0}>
            <section aria-label={t('factoryDashboard.waitingOnYou')}>
              <SectionHeading
                title={t('factoryDashboard.waitingOnYou')}
                count={waiting().length}
                tone="warning"
              />
              <ul class="flex flex-col gap-2">
                <For each={waiting()}>
                  {(listed) => (
                    <GateItem
                      listed={listed}
                      owned={listed.run.ownerId === props.viewerId}
                      age={age}
                      onOpen={() => props.onOpenRun(listed.run.id)}
                      onAnswer={(step, answer) =>
                        props.onAnswer({ runId: listed.run.id, step, answer })
                      }
                    />
                  )}
                </For>
              </ul>
            </section>
          </Show>
          <section aria-label={t('factoryDashboard.activeRuns')}>
            <SectionHeading
              title={t('factoryDashboard.activeRuns')}
              count={active().length}
              tone="accent"
            />
            <Switch>
              <Match when={props.data.active.state() === 'pending'}>
                <Skeleton rows={2} tall />
              </Match>
              <Match when={props.data.active.state() === 'error'}>
                <div class="flex items-center gap-2 text-sm text-ink-muted">
                  {t('factoryDashboard.loadFailed')}
                  <Button
                    size="xs"
                    variant="ghost"
                    onClick={() => props.data.active.retry()}
                  >
                    {t('common.retry')}
                  </Button>
                </div>
              </Match>
              <Match when={active().length === 0}>
                <div class="flex flex-col items-center gap-2 rounded-xl border border-dashed border-edge-muted px-6 py-10 text-center">
                  <CheckCircleIcon class="size-6 text-ink-subtle" />
                  <p class="text-sm font-medium text-ink">
                    {t('factoryDashboard.nothingRunning')}
                  </p>
                  <p class="max-w-sm text-xs text-ink-muted">
                    {t('factoryDashboard.nothingRunningHint')}
                  </p>
                </div>
              </Match>
              <Match when={active().length > 0}>
                <ul class="flex flex-col gap-3">
                  <For each={active()}>
                    {(listed) => (
                      <ActiveRunCard
                        listed={listed}
                        entries={props.entries}
                        now={props.now}
                        onOpen={() => props.onOpenRun(listed.run.id)}
                      />
                    )}
                  </For>
                </ul>
              </Match>
            </Switch>
          </section>
        </div>
        <aside class="flex min-w-0 flex-col gap-7">
          <Switch>
            <Match when={props.data.queue.data()}>
              {(queue) => (
                <SlotsPanel
                  places={queue().places}
                  limit={props.data.queueLimit.data()}
                />
              )}
            </Match>
            <Match when={props.data.queue.state() === 'pending'}>
              <Skeleton rows={2} />
            </Match>
          </Switch>
          <Switch>
            <Match when={props.data.recent.state() === 'pending'}>
              <Skeleton rows={5} />
            </Match>
            <Match when={true}>
              <RecentPanel runs={ended()} age={age} onOpen={props.onOpenRun} />
            </Match>
          </Switch>
        </aside>
      </div>
    </div>
  );
}

/** Says what went wrong with an answer the dashboard sent. */
export function notifyDashboardAnswer(
  result: { ok: true } | { ok: false; reason: 'conflict' | 'failed' }
): boolean {
  if (result.ok) return true;
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.answerConflict'), {
      subtext: t('factoryRuns.answerConflictDescription'),
    });
  } else {
    toast.failure(t('factoryRuns.answerFailed'));
  }
  return false;
}
