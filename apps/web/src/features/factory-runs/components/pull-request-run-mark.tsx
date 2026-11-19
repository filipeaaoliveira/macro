import { t } from '@app/lib/i18n';
import ArrowSquareOutIcon from '@icon/system/regular/arrow-square-out.svg';
import { Badge, Button } from '@ui';
import { Show } from 'solid-js';
import { match } from 'ts-pattern';
import { currentStep, openBlockers } from '../core/board';
import { flowWordLabel } from '../core/flow-words';
import {
  latestReviewRound,
  type PullRequestMerge,
  type PullRequestRun,
} from '../core/pull-request-run';
import { runOutcomeLabel } from './run-status';

/**
 * What merging the pull request does now, when its Run guards it: its
 * merge Gate takes it, or why it takes none. None when no Run guards it.
 */
export function pullRequestMergeNote(
  merge: PullRequestMerge,
  ownerName: string | undefined
): string | undefined {
  return match(merge)
    .with({ kind: 'github' }, (github) =>
      github.stopped
        ? t('factoryBoard.mergeRunStopped', {
            outcome: runOutcomeLabel(github.stopped),
          })
        : undefined
    )
    .with({ kind: 'gate' }, () => t('factoryBoard.mergeAtGate'))
    .with({ kind: 'refused', why: 'checking' }, () =>
      t('factoryBoard.mergeChecking')
    )
    .with({ kind: 'refused', why: 'notOwner' }, () =>
      ownerName
        ? t('factoryBoard.mergeOnlyOwner', { owner: ownerName })
        : t('factoryBoard.mergeOnlyTheOwner')
    )
    .with({ kind: 'refused', why: 'held' }, (held) =>
      t('factoryBoard.mergeHeld', { branch: held.branch })
    )
    .with({ kind: 'refused', why: 'notAtGate' }, (refused) => {
      if (!refused.step) return t('factoryBoard.mergeNotAtGate');
      const step = flowWordLabel(refused.step.entry);
      return refused.waiting
        ? t('factoryBoard.mergeAfterOtherGate', { step })
        : t('factoryBoard.mergeNotAtGateStep', { step });
    })
    .exhaustive();
}

/**
 * Where the Run that opened a pull request is, in a few words: who it waits
 * on, what holds it, its Step, or how it ended.
 */
function runPlace(
  found: PullRequestRun,
  ownerName: string | undefined
): string {
  const run = found.run;
  const owner = ownerName ?? t('factoryBoard.someone');
  if (run.end) return runOutcomeLabel(run.end.outcome);
  if (found.state === 'waiting') {
    return found.mergeGateSeq === undefined
      ? t('factoryBoard.waitingOn', { owner })
      : t('factoryBoard.waitingToMerge', { owner });
  }
  if (found.state === 'held') {
    return t('factoryBoard.heldBy', {
      branch: run.repository?.baseBranch ?? '',
    });
  }
  const step = currentStep(run);
  return step ? flowWordLabel(step.entry) : t('factoryBoard.stateRunning');
}

/**
 * The mark of a pull request a factory Run opened, as a Reviews card shows
 * it: the factory's, where its Run is, its latest review round and open
 * Blockers, a link to the Run, and what merging it does now.
 */
export function PullRequestRunMark(props: {
  found: PullRequestRun;
  merge: PullRequestMerge;
  /** The Run's owner as the viewer reads them: `you`, their name, or none. */
  ownerName?: string;
  onOpenRun?: (runId: string) => void;
}) {
  const round = () => latestReviewRound(props.found);
  const blockers = () => openBlockers(props.found.run);
  const note = () => pullRequestMergeNote(props.merge, props.ownerName);

  return (
    <div
      data-pull-request-run={props.found.run.id}
      class="flex min-w-0 flex-col gap-1 text-xs"
    >
      <div class="flex min-w-0 items-center gap-1.5">
        <Badge variant="outline" size="xs">
          {t('factoryBoard.pullRequestMark')}
        </Badge>
        <span class="min-w-0 truncate text-ink-muted">
          {runPlace(props.found, props.ownerName)}
        </span>
        <Show when={round()}>
          {(number) => (
            <span class="shrink-0 text-ink-subtle">
              {t('factoryBoard.reviewRound', { round: number() })}
            </span>
          )}
        </Show>
        <Show when={blockers() > 0}>
          <Badge variant="failure" size="xs">
            {t('factoryBoard.blockers', { count: blockers() })}
          </Badge>
        </Show>
        <Show when={props.onOpenRun}>
          {(openRun) => (
            <Button
              variant="ghost"
              size="icon-xs"
              class="ml-auto shrink-0"
              aria-label={t('factoryBoard.openRun')}
              tooltip={t('factoryBoard.openRun')}
              onClick={() => openRun()(props.found.run.id)}
            >
              <ArrowSquareOutIcon class="size-3 touch:size-6 shrink-0 pointer-events-none" />
            </Button>
          )}
        </Show>
      </div>
      <Show when={note()}>
        {(text) => <p class="text-ink-subtle">{text()}</p>}
      </Show>
    </div>
  );
}
