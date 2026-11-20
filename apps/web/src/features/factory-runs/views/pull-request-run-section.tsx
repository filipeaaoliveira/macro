import { t } from '@app/lib/i18n';
import { SidePanel } from '@components/app/side-panel';
import { Button } from '@ui';
import { Show } from 'solid-js';
import { PullRequestRunMark } from '../components/pull-request-run-mark';
import { ReviewRoundList } from '../components/run-timeline';
import type {
  PullRequestMerge,
  PullRequestRun,
} from '../core/pull-request-run';

/**
 * The Run that opened a pull request, as a section of the pull request's
 * side panel: where it is, what merging does now, and its review rounds
 * with what their reviewers found, as the Run's own view shows them. Its
 * owner answers its merge Gate from here while the Run waits there.
 */
export function PullRequestRunSection(props: {
  found: PullRequestRun;
  merge: PullRequestMerge;
  ownerName?: string;
  order: number;
  onOpenRun?: (runId: string) => void;
  onAnswerGate: (runId: string) => void;
}) {
  const rounds = () => props.found.run.reviewRounds ?? [];

  return (
    <SidePanel.Section
      id="factory-run"
      title={t('factoryBoard.pullRequestRun')}
      defaultOpen
      order={props.order}
    >
      <div class="flex min-w-0 flex-col gap-3">
        <PullRequestRunMark
          found={props.found}
          merge={props.merge}
          ownerName={props.ownerName}
          onOpenRun={props.onOpenRun}
        />
        <Show when={props.merge.kind === 'gate' ? props.merge : undefined}>
          {(gate) => (
            <Button
              variant="secondary"
              size="xs"
              class="self-start"
              onClick={() => props.onAnswerGate(gate().runId)}
            >
              {t('factoryBoard.answerMergeGate')}
            </Button>
          )}
        </Show>
        <Show
          when={rounds().length > 0}
          fallback={
            <p class="text-xs text-ink-placeholder">
              {t('factoryBoard.noReviewRounds')}
            </p>
          }
        >
          <div class="min-w-0">
            <ReviewRoundList rounds={rounds()} steps={props.found.run.steps} />
          </div>
        </Show>
      </div>
    </SidePanel.Section>
  );
}
