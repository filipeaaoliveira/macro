import {
  WorkDot,
  type WorkDotState,
} from '@app/features/agent-work/components/WorkList';
import { t } from '@app/lib/i18n';
import GitPullRequestIcon from '@icon/system/regular/git-pull-request.svg';
import { Badge } from '@ui';
import { Show } from 'solid-js';
import { match } from 'ts-pattern';
import { useBoardContext } from '../context/board-context';
import {
  type BoardRun,
  type BoardState,
  currentStep,
  openBlockers,
  repositoryName,
  startedBy,
  stepSince,
} from '../core/board';
import { flowWordLabel } from '../core/flow-words';
import { runTitle } from '../core/run';

/** A board state as Work's status dot. */
function stateDot(state: BoardState): WorkDotState {
  return match(state)
    .with('running', () => 'running' as const)
    .with('waiting', 'held', () => 'needs_input' as const)
    .with('stopped', () => 'failed' as const)
    .with('finished', () => 'done' as const)
    .exhaustive();
}

/**
 * One Run on the board: its task, where it is (its Step, the Role and Agent
 * on it, or who it waits on), whose it is and who started it, how long it
 * has been there, its open Blockers and its pull request. The row opens
 * the Run.
 */
export function BoardRunRow(props: {
  listed: BoardRun;
  /** How long ago a time was, compact: `5m`. */
  age: (time: string) => string;
  onOpen: (runId: string) => void;
}) {
  const context = useBoardContext();
  const run = () => props.listed.run;
  const ownerName = context.personName(() => run().ownerId ?? '');
  const step = () => currentStep(run());
  const blockers = () => openBlockers(run());

  const owner = () => {
    if (run().ownerId && run().ownerId === context.viewerId()) {
      return t('factoryBoard.you');
    }
    return ownerName() ?? t('factoryBoard.someone');
  };
  const title = () => props.listed.taskName ?? runTitle(run());
  const starter = () =>
    match(startedBy(run()))
      .with('person', () =>
        t('factoryBoard.startedByPerson', { owner: owner() })
      )
      .with('orchestrator', () =>
        t('factoryBoard.startedByOrchestrator', { owner: owner() })
      )
      .with('automation', () =>
        t('factoryBoard.startedByAutomation', {
          name: run().automation?.name ?? '',
        })
      )
      .exhaustive();
  // Where the Run is now: who it waits on, what holds it, or its Step and
  // the agent on it.
  const where = () => {
    const current = step();
    if (props.listed.state === 'waiting') {
      return t('factoryBoard.waitingOn', { owner: owner() });
    }
    if (props.listed.state === 'held') {
      return t('factoryBoard.heldBy', {
        branch: run().repository?.baseBranch ?? '',
      });
    }
    if (!current) return undefined;
    const name = flowWordLabel(current.entry);
    if (current.role && current.agent) {
      return t('factoryBoard.stepWithAgent', {
        step: name,
        role: current.role,
        agent: current.agent,
      });
    }
    if (current.role) {
      return t('factoryBoard.stepWithRole', { step: name, role: current.role });
    }
    return name;
  };
  const since = () =>
    run().end ? run().end?.endedAt : stepSince(run(), step());
  const details = () =>
    [
      run().repository
        ? repositoryName(run().repository?.url ?? '')
        : undefined,
      run().flow ? flowWordLabel(run().flow ?? '') : undefined,
      starter(),
    ].filter((part) => part !== undefined);

  return (
    <li class="relative flex items-start gap-1 rounded-lg not-touch:hover:bg-hover not-touch:focus-within:bg-hover">
      <button
        type="button"
        data-board-run={run().id}
        class="flex min-w-0 flex-1 items-start gap-2 rounded-lg px-2 py-1.5 text-left outline-none focus-visible:outline-2 focus-visible:outline-accent touch:min-h-11"
        onClick={() => props.onOpen(run().id)}
      >
        <span class="flex h-4.5 shrink-0 items-center">
          <WorkDot state={stateDot(props.listed.state)} />
        </span>
        <span class="flex min-w-0 flex-1 flex-col">
          <span class="truncate text-[13px] leading-[18px] font-medium text-ink">
            {title()}
          </span>
          <Show when={where()}>
            {(text) => (
              <span class="truncate text-[11.5px] leading-4 text-ink-muted">
                {text()}
              </span>
            )}
          </Show>
          <span class="truncate text-[11.5px] leading-4 text-ink-subtle">
            {details().join(' · ')}
          </span>
        </span>
        <span class="flex h-4.5 shrink-0 items-center gap-1.5 text-[11.5px] tabular-nums text-ink-subtle">
          <Show when={blockers() > 0}>
            <Badge
              variant="outline"
              size="xs"
              class="border-failure/30 text-failure"
            >
              {t('factoryBoard.blockers', { count: blockers() })}
            </Badge>
          </Show>
          <Show when={since()}>{(time) => props.age(time())}</Show>
        </span>
      </button>
      <Show when={run().pullRequestUrl}>
        {(url) => (
          <a
            href={url()}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={t('factoryBoard.pullRequest')}
            title={t('factoryBoard.pullRequest')}
            class="mt-1 flex size-7 shrink-0 items-center justify-center rounded-md text-ink-subtle outline-none hover:bg-active hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
          >
            <GitPullRequestIcon aria-hidden="true" class="size-3.5" />
          </a>
        )}
      </Show>
    </li>
  );
}
