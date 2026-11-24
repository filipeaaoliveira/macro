import { t } from '@app/lib/i18n';
import ChatIcon from '@icon/system/regular/chat-circle-text.svg';
import CheckIcon from '@icon/system/regular/check.svg';
import CircleIcon from '@icon/system/regular/circle.svg';
import CodeIcon from '@icon/system/regular/code.svg';
import CpuIcon from '@icon/system/regular/cpu.svg';
import DesktopIcon from '@icon/system/regular/desktop.svg';
import FileIcon from '@icon/system/regular/file-text.svg';
import FlaskIcon from '@icon/system/regular/flask.svg';
import GearIcon from '@icon/system/regular/gear-six.svg';
import GitMergeIcon from '@icon/system/regular/git-merge.svg';
import GitPullRequestIcon from '@icon/system/regular/git-pull-request.svg';
import HandIcon from '@icon/system/regular/hand.svg';
import ListNumbersIcon from '@icon/system/regular/list-numbers.svg';
import MagnifyingGlassIcon from '@icon/system/regular/magnifying-glass.svg';
import MinusIcon from '@icon/system/regular/minus-circle.svg';
import PencilIcon from '@icon/system/regular/pencil-simple.svg';
import RepeatIcon from '@icon/system/regular/repeat.svg';
import ShieldCheckIcon from '@icon/system/regular/shield-check.svg';
import TerminalIcon from '@icon/system/regular/terminal.svg';
import UsersThreeIcon from '@icon/system/regular/users-three.svg';
import WrenchIcon from '@icon/system/regular/wrench.svg';
import XIcon from '@icon/system/regular/x.svg';
import { cn } from '@ui';
import { type Component, For, type JSX, Match, Show, Switch } from 'solid-js';
import { Dynamic } from 'solid-js/web';
import { useRunsContext } from '../context/runs-context';
import type { ActivityLine } from '../core/activity';
import { flowWordLabel } from '../core/flow-words';
import {
  elapsedMs,
  formatElapsed,
  type PipelineStage,
  type StageState,
  stepFellShort,
  stepRuntime,
} from '../core/live';
import { type RunStep, steerableSession } from '../core/run';
import './live.css';

type Icon = Component<JSX.SvgSVGAttributes<SVGSVGElement>>;

/** The icon of a Flow's entry; a plain circle for one it does not know. */
const STAGE_ICONS: Record<string, Icon> = {
  plan: ListNumbersIcon,
  'plan-review': MagnifyingGlassIcon,
  write: CodeIcon,
  review: UsersThreeIcon,
  fix: WrenchIcon,
  accept: ShieldCheckIcon,
  publish: GitPullRequestIcon,
  ci: FlaskIcon,
  'merge-gate': HandIcon,
  merge: GitMergeIcon,
};

export function stageIcon(entry: string): Icon {
  return STAGE_ICONS[entry] ?? CircleIcon;
}

/** A stage's state, for a screen reader and a tooltip. */
export function stageStateLabel(state: StageState): string {
  switch (state) {
    case 'done':
      return t('factoryDashboard.stageDone');
    case 'current':
      return t('factoryDashboard.stageCurrent');
    case 'waiting':
      return t('factoryDashboard.stageWaiting');
    case 'failed':
      return t('factoryDashboard.stageFailed');
    case 'stopped':
      return t('factoryDashboard.stageStopped');
    case 'skipped':
      return t('factoryDashboard.stageSkipped');
    case 'pending':
      return t('factoryDashboard.stagePending');
  }
}

const BAR: Record<StageState, string> = {
  done: 'bg-success/70',
  current: 'bg-accent',
  waiting: 'bg-warning factory-live-breathe',
  failed: 'bg-failure',
  stopped: 'bg-ink-disabled',
  skipped: 'border border-dashed border-edge-muted bg-transparent',
  pending: 'bg-edge-muted',
};

const LABEL: Record<StageState, string> = {
  done: 'text-ink-muted',
  current: 'font-semibold text-accent',
  waiting: 'font-semibold text-warning',
  failed: 'font-semibold text-failure',
  stopped: 'text-ink-muted',
  skipped: 'text-ink-extra-muted',
  pending: 'text-ink-extra-muted',
};

/**
 * A Run's stages as one compact track: a bar per stage in its state's
 * colour, its name under it, the working stage's bar lit and moving.
 */
export function StageTrack(props: { stages: PipelineStage[] }) {
  return (
    <ol
      class="grid gap-1"
      style={{
        'grid-template-columns': `repeat(${props.stages.length}, minmax(0, 1fr))`,
      }}
    >
      <For each={props.stages}>
        {(stage) => (
          <li
            class="flex min-w-0 flex-col gap-1.5"
            title={`${flowWordLabel(stage.entry)} · ${stageStateLabel(stage.state)}`}
          >
            <span
              class={cn(
                'relative h-1.5 overflow-hidden rounded-full',
                BAR[stage.state]
              )}
            >
              <Show when={stage.state === 'current'}>
                <span class="factory-live-sheen" />
              </Show>
            </span>
            <span
              class={cn(
                'flex items-center gap-1 truncate text-[11px] leading-3.5',
                LABEL[stage.state]
              )}
            >
              <span class="truncate">{flowWordLabel(stage.entry)}</span>
              <Show when={stage.visits > 1}>
                <span class="shrink-0 tabular-nums text-ink-subtle">
                  ×{stage.visits}
                </span>
              </Show>
            </span>
            <span class="sr-only">{stageStateLabel(stage.state)}</span>
          </li>
        )}
      </For>
    </ol>
  );
}

/** The tones of a Role's avatar: who it is, apart from how it is doing. */
const ROLE_TONES = [
  'bg-violet-bg text-violet-ink',
  'bg-teal-bg text-teal-ink',
  'bg-blue-bg text-blue-ink',
  'bg-pink-bg text-pink-ink',
  'bg-orange-bg text-orange-ink',
  'bg-lime-bg text-lime-ink',
  'bg-cyan-bg text-cyan-ink',
];

function roleTone(role: string): string {
  let hash = 0;
  for (const char of role) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return ROLE_TONES[hash % ROLE_TONES.length] ?? ROLE_TONES[0];
}

/** The two letters of a Role: `plan-reviewer` reads `PR`, `writer` `WR`. */
function roleInitials(role: string): string {
  const parts = role.split(/[-_\s]+/).filter(Boolean);
  if (parts.length > 1) {
    return `${parts[0]?.[0] ?? ''}${parts[1]?.[0] ?? ''}`.toUpperCase();
  }
  return role.slice(0, 2).toUpperCase();
}

export type AgentStatus = 'working' | 'waiting' | 'done' | 'failed';

/**
 * A Role as an avatar: its own colour and letters, and a badge for how its
 * Step is going, so who works and how it goes read apart. A Step with no
 * Role is the factory's own.
 */
export function RoleAvatar(props: {
  role?: string;
  /** A review panel: several reviewers at once. */
  panel?: boolean;
  status: AgentStatus;
  size?: 'sm' | 'md' | 'lg';
}) {
  const size = () => props.size ?? 'md';
  return (
    <span
      class={cn(
        'relative inline-flex shrink-0 items-center justify-center rounded-full font-semibold tracking-tight',
        size() === 'sm' && 'size-6 text-[9.5px]',
        size() === 'md' && 'size-8 text-[11px]',
        size() === 'lg' && 'size-10 text-[13px]',
        props.panel
          ? 'bg-accent-bg text-accent'
          : props.role
            ? roleTone(props.role)
            : 'bg-surface-3 text-ink-muted'
      )}
      aria-hidden="true"
    >
      <Switch>
        <Match when={props.panel}>
          <UsersThreeIcon class="size-[55%]" />
        </Match>
        <Match when={props.role}>{(role) => roleInitials(role())}</Match>
        <Match when={true}>
          <GearIcon class="size-[55%]" />
        </Match>
      </Switch>
      <span
        class={cn(
          'absolute -right-0.5 -bottom-0.5 rounded-full ring-2 ring-surface',
          size() === 'lg' ? 'size-3' : 'size-2.5',
          props.status === 'working' && 'bg-accent',
          props.status === 'waiting' && 'bg-warning',
          props.status === 'done' && 'bg-success',
          props.status === 'failed' && 'bg-failure'
        )}
      >
        <Show when={props.status === 'working'}>
          <span class="factory-live-ripple text-accent" />
        </Show>
      </span>
    </span>
  );
}

/**
 * Where a Step runs: its model and its Machine, each only once the API says
 * (`stepRuntime`); nothing in their place until then.
 */
export function RuntimeChips(props: { step: RunStep; class?: string }) {
  const runtime = () => stepRuntime(props.step);
  const chips = () =>
    [
      { icon: CpuIcon, value: runtime().model },
      { icon: DesktopIcon, value: runtime().machine },
    ].filter((chip): chip is { icon: Icon; value: string } => !!chip.value);
  return (
    <Show when={chips().length > 0}>
      <span class={cn('flex flex-wrap items-center gap-1', props.class)}>
        <For each={chips()}>
          {(chip) => (
            <span class="inline-flex h-5 max-w-40 items-center gap-1 rounded-md bg-surface-2 px-1.5 text-[11px] text-ink-muted">
              <Dynamic component={chip.icon} class="size-3 shrink-0" />
              <span class="truncate">{chip.value}</span>
            </span>
          )}
        </For>
      </span>
    </Show>
  );
}

/** A reviewer of a panel: its Role without `-reviewer`, `correctness`. */
export function reviewerName(role: string | undefined): string {
  if (!role) return t('factoryDashboard.reviewer');
  return flowWordLabel(role.replace(/-?reviewer$/, '') || role);
}

/** How one Step went, as a reviewer pill and an agent badge show it. */
export function stepStatus(step: RunStep): AgentStatus {
  if (!step.end) return 'working';
  if (step.gate && !step.gate.answer) return 'waiting';
  if (stepFellShort(step) || step.end.outcome === 'changes') return 'failed';
  return 'done';
}

/** A review panel's reviewers at once: each its own pill and state. */
export function ReviewerPills(props: { steps: RunStep[]; vertical?: boolean }) {
  return (
    <ul
      class={cn(
        'flex gap-1',
        props.vertical ? 'flex-col items-stretch' : 'flex-wrap items-center'
      )}
      aria-label={t('factoryDashboard.reviewers')}
    >
      <For each={props.steps}>
        {(step) => {
          const status = () => stepStatus(step);
          return (
            <li
              class={cn(
                'inline-flex h-6 min-w-0 items-center gap-1.5 rounded-full border pr-2.5 pl-1 text-[11.5px]',
                status() === 'working' &&
                  'border-accent/40 bg-accent-bg text-ink',
                status() === 'done' &&
                  'border-transparent bg-success-bg text-ink-muted',
                status() === 'failed' &&
                  'border-transparent bg-warning-bg text-ink-muted',
                status() === 'waiting' &&
                  'border-transparent bg-warning-bg text-ink'
              )}
            >
              <span
                class={cn(
                  'relative flex size-4 shrink-0 items-center justify-center rounded-full',
                  status() === 'working' && 'text-accent',
                  status() === 'done' && 'text-success',
                  status() !== 'working' &&
                    status() !== 'done' &&
                    'text-warning'
                )}
              >
                <Switch>
                  <Match when={status() === 'working'}>
                    <span class="size-1.5 rounded-full bg-accent" />
                    <span class="factory-live-ripple" />
                  </Match>
                  <Match when={status() === 'done'}>
                    <CheckIcon class="size-3" />
                  </Match>
                  <Match when={true}>
                    <XIcon class="size-3" />
                  </Match>
                </Switch>
              </span>
              <span class="truncate">{reviewerName(step.role)}</span>
              <span class="sr-only">
                {status() === 'working'
                  ? t('factoryDashboard.reviewing')
                  : flowWordLabel(step.end?.outcome ?? '')}
              </span>
            </li>
          );
        }}
      </For>
    </ul>
  );
}

const NODE: Record<StageState, string> = {
  done: 'bg-success-bg text-success border-transparent',
  current:
    'bg-accent text-accent-contrast border-transparent shadow-[0_6px_20px_-6px_var(--color-accent)]',
  waiting:
    'bg-warning-bg text-warning border-warning/60 shadow-[0_6px_20px_-8px_var(--color-warning)]',
  failed: 'bg-failure-bg text-failure border-failure/50',
  stopped: 'bg-surface-2 text-ink-muted border-edge-muted',
  skipped:
    'bg-transparent text-ink-extra-muted border-dashed border-edge-muted',
  pending: 'bg-surface-1 text-ink-extra-muted border-edge-muted',
};

/** The line into a stage takes the colour of the stage it leaves. */
function lineTone(state: StageState | undefined): string {
  if (state === 'done') return 'bg-success/50';
  if (state === 'current' || state === 'waiting') return 'bg-edge-muted';
  return 'bg-edge-muted';
}

/** A loop drawn above the stages it goes back over. */
type Loop = { from: number; to: number; label: string };

function loopsOf(stages: PipelineStage[]): Loop[] {
  const loops: Loop[] = [];
  const index = (entry: string) =>
    stages.findIndex((stage) => stage.entry === entry);
  const review = index('review');
  const fix = index('fix');
  const reviewStage = stages[review];
  const fixStage = stages[fix];
  if (reviewStage && fixStage && fixStage.visits > 0) {
    loops.push({
      from: Math.min(review, fix),
      to: Math.max(review, fix),
      label: t('factoryDashboard.reviewRounds', {
        count: reviewStage.visits,
      }),
    });
  }
  const plan = index('plan');
  const planReview = index('plan-review');
  const planStage = stages[plan];
  if (planStage && planReview >= 0 && planStage.visits > 1) {
    loops.push({
      from: Math.min(plan, planReview),
      to: Math.max(plan, planReview),
      label: t('factoryDashboard.planRounds', { count: planStage.visits }),
    });
  }
  return loops;
}

/** How long a stage took, or has taken so far. */
function stageTime(stage: PipelineStage, now: number): string | undefined {
  if (!stage.startedAt) return undefined;
  if (stage.state === 'pending' || stage.state === 'skipped') return undefined;
  return formatElapsed(elapsedMs(stage.startedAt, stage.endedAt, now));
}

/**
 * A Run's stages, large: a node per stage on one line, the working one lit,
 * with a ripple, Gates marked, loops drawn over the stages they go back to,
 * and under the review its panel's reviewers.
 */
export function StagePipeline(props: { stages: PipelineStage[]; now: number }) {
  const columns = () => `repeat(${props.stages.length}, minmax(0, 1fr))`;
  const loops = () => loopsOf(props.stages);

  return (
    <div class="overflow-x-auto pb-1">
      <div class="relative min-w-[720px]">
        <Show when={loops().length > 0}>
          <div class="grid h-7" style={{ 'grid-template-columns': columns() }}>
            <For each={loops()}>
              {(loop) => (
                <div
                  class="relative"
                  style={{
                    'grid-column': `${loop.from + 1} / ${loop.to + 2}`,
                    'grid-row': '1',
                  }}
                >
                  <div
                    class="absolute top-3 bottom-[-6px] rounded-t-xl border-x border-t border-dashed border-warning/70"
                    style={{
                      left: `${50 / (loop.to - loop.from + 1)}%`,
                      right: `${50 / (loop.to - loop.from + 1)}%`,
                    }}
                  />
                  <span class="absolute top-0 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full bg-surface px-1.5 text-[11px] whitespace-nowrap text-warning">
                    <RepeatIcon class="size-3" />
                    {loop.label}
                  </span>
                </div>
              )}
            </For>
          </div>
        </Show>
        <ol class="grid" style={{ 'grid-template-columns': columns() }}>
          <For each={props.stages}>
            {(stage, index) => {
              const previous = () => props.stages[index() - 1];
              const live = () =>
                stage.state === 'current' || stage.state === 'waiting';
              return (
                <li
                  class="flex min-w-0 flex-col items-center"
                  aria-current={live() ? 'step' : undefined}
                >
                  <div class="relative flex h-13 w-full items-center justify-center">
                    <Show when={index() > 0}>
                      <span
                        class={cn(
                          'absolute top-1/2 left-0 h-0.5 w-1/2 -translate-y-1/2',
                          lineTone(previous()?.state)
                        )}
                      />
                    </Show>
                    <Show when={index() < props.stages.length - 1}>
                      <span
                        class={cn(
                          'absolute top-1/2 right-0 h-0.5 w-1/2 -translate-y-1/2',
                          lineTone(stage.state)
                        )}
                      />
                    </Show>
                    <span
                      class={cn(
                        'relative z-1 flex items-center justify-center rounded-full border transition-colors duration-300',
                        live() ? 'size-12' : 'size-10',
                        NODE[stage.state]
                      )}
                    >
                      <Show when={live()}>
                        <span
                          class={cn(
                            'factory-live-ripple',
                            stage.state === 'current'
                              ? 'text-accent'
                              : 'text-warning'
                          )}
                        />
                      </Show>
                      <Switch>
                        <Match when={stage.state === 'done'}>
                          <CheckIcon class="size-4.5" />
                        </Match>
                        <Match when={stage.state === 'failed'}>
                          <XIcon class="size-4.5" />
                        </Match>
                        <Match when={stage.state === 'skipped'}>
                          <MinusIcon class="size-4" />
                        </Match>
                        <Match when={true}>
                          <Dynamic
                            component={stageIcon(stage.entry)}
                            class={live() ? 'size-5.5' : 'size-4.5'}
                          />
                        </Match>
                      </Switch>
                      <Show when={stage.gate}>
                        {(gate) => (
                          <span
                            class={cn(
                              'absolute -top-1 -right-1 flex size-4.5 items-center justify-center rounded-full ring-2 ring-surface',
                              gate().open
                                ? 'bg-warning text-page'
                                : 'bg-surface-3 text-ink-muted'
                            )}
                            title={t('factoryDashboard.gateHere')}
                          >
                            <HandIcon class="size-2.5" />
                          </span>
                        )}
                      </Show>
                    </span>
                  </div>
                  <span
                    class={cn(
                      'mt-1.5 max-w-full truncate px-1 text-center text-[12.5px] leading-4',
                      LABEL[stage.state]
                    )}
                  >
                    {flowWordLabel(stage.entry)}
                  </span>
                  <span class="mt-0.5 flex h-4 items-center gap-1.5 text-[11px] tabular-nums text-ink-subtle">
                    <Show
                      when={stageTime(stage, props.now)}
                      fallback={
                        <span class="text-ink-extra-muted">
                          {stageStateLabel(stage.state)}
                        </span>
                      }
                    >
                      {(time) => <span>{time()}</span>}
                    </Show>
                    <For each={stage.loops}>
                      {(loop) => (
                        <span
                          class="inline-flex items-center gap-0.5 text-warning"
                          title={flowWordLabel(loop.entry)}
                        >
                          <RepeatIcon class="size-3" />
                          {loop.count}
                        </span>
                      )}
                    </For>
                  </span>
                  <Show when={stage.latest.length > 1}>
                    <span class="mt-1 flex items-center gap-0.5">
                      <For each={stage.latest}>
                        {(step) => (
                          <span
                            class={cn(
                              'size-1.5 rounded-full',
                              stepStatus(step) === 'working' && 'bg-accent',
                              stepStatus(step) === 'done' && 'bg-success',
                              stepStatus(step) !== 'working' &&
                                stepStatus(step) !== 'done' &&
                                'bg-warning'
                            )}
                            title={reviewerName(step.role)}
                          />
                        )}
                      </For>
                    </span>
                  </Show>
                </li>
              );
            }}
          </For>
        </ol>
      </div>
    </div>
  );
}

const ACTIVITY_ICONS: Record<ActivityLine['kind'], Icon> = {
  edit: PencilIcon,
  read: FileIcon,
  run: TerminalIcon,
  search: MagnifyingGlassIcon,
  say: ChatIcon,
};

function activityText(line: ActivityLine): string {
  switch (line.kind) {
    case 'edit':
      return t('factoryDashboard.activityEdit', { path: line.text });
    case 'read':
      return t('factoryDashboard.activityRead', { path: line.text });
    case 'run':
      return t('factoryDashboard.activityRun', { command: line.text });
    case 'search':
      return t('factoryDashboard.activitySearch', { path: line.text });
    case 'say':
      return line.text;
  }
}

/**
 * What a running Step's agent did last, newest at the bottom and pulsing:
 * the tail of the session its Step Terminal shows. `compact` shows only the
 * newest, on one line. Nothing while the session says nothing.
 */
export function ActivityTail(props: {
  step: RunStep;
  limit?: number;
  compact?: boolean;
  class?: string;
}) {
  const context = useRunsContext();
  const lines =
    context.stepActivity?.(
      () => steerableSession(props.step),
      props.compact ? 1 : (props.limit ?? 4)
    ) ?? (() => []);
  return (
    <Show when={lines().length > 0}>
      <ol
        class={cn('flex min-w-0 flex-col gap-1', props.class)}
        aria-live="polite"
        aria-label={t('factoryDashboard.activity')}
      >
        <For each={lines()}>
          {(line, index) => {
            const newest = () => index() === lines().length - 1;
            return (
              <li
                class={cn(
                  'factory-live-enter flex min-w-0 items-center gap-2 text-[12px] leading-4.5',
                  newest() ? 'text-ink' : 'text-ink-subtle'
                )}
              >
                <span
                  class={cn(
                    'relative flex size-4 shrink-0 items-center justify-center',
                    newest() ? 'text-accent' : 'text-ink-extra-muted'
                  )}
                >
                  <Dynamic
                    component={ACTIVITY_ICONS[line.kind]}
                    class="size-3.5"
                  />
                </span>
                <span class="min-w-0 truncate">{activityText(line)}</span>
                <Show when={line.kind === 'run' && line.exitCode !== undefined}>
                  <span
                    class={cn(
                      'shrink-0 rounded px-1 text-[10.5px] tabular-nums',
                      line.failed
                        ? 'bg-failure-bg text-failure'
                        : 'bg-success-bg text-success'
                    )}
                  >
                    {t('factoryDashboard.exitCode', {
                      code: line.exitCode ?? 0,
                    })}
                  </span>
                </Show>
                <Show when={newest() && line.live}>
                  <span class="factory-live-breathe size-1.5 shrink-0 rounded-full bg-accent" />
                </Show>
              </li>
            );
          }}
        </For>
      </ol>
    </Show>
  );
}
