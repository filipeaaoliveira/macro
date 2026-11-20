import { fireAndForget } from '@core/util/fireAndForget';
import GitMerge from '@icon/system/regular/git-merge.svg';
import { Button, ButtonGroup, type ButtonSize, cn, confirmDialog } from '@ui';
import { getOwner, type Owner, Show } from 'solid-js';
import {
  createMergePullRequestAction,
  type MergePullRequestTarget,
} from '../primitives/create-merge-pull-request-action';
import { usePrMergeGuard } from '../primitives/pr-merge-guard-context';
import {
  type PullRequestCheck,
  pullRequestReadyToMerge,
} from '../primitives/pull-request-ready-to-merge';
import { prDisplayName, prHtmlUrl } from '../util/prKey';

/**
 * Ask before merging, naming the pull request, and what the person should
 * know first (`warning`), if anything.
 */
export function confirmMergePullRequest(
  target: MergePullRequestTarget & { warning?: string },
  owner: Owner | null
) {
  return confirmDialog(
    {
      title: 'Merge pull request?',
      get body() {
        return (
          <div class="space-y-4 pt-2">
            <div class="flex items-start gap-3 rounded-lg border border-edge-muted bg-input p-3">
              <GitMerge
                aria-hidden="true"
                class="mt-0.5 size-4 shrink-0 text-ink-muted"
              />
              <div class="min-w-0 space-y-1">
                <Show when={target.title}>
                  <p class="text-sm font-medium text-ink wrap-anywhere">
                    {target.title}
                  </p>
                </Show>
                <p class="text-xs text-ink-muted wrap-anywhere">
                  {prDisplayName(target)}
                </p>
              </div>
            </div>
            <Show when={target.warning}>
              {(warning) => (
                <p class="font-medium text-failure-ink">{warning()}</p>
              )}
            </Show>
            <p>
              This will merge the pull request into its base branch on GitHub.
            </p>
          </div>
        );
      },
      confirmLabel: 'Merge pull request',
    },
    { owner }
  );
}

/**
 * Merge an open pull request as the signed-in user. Renders nothing unless
 * the pull request is open, not a draft, and its checks have finished
 * without failing. GitHub's own permissions decide whether the merge goes
 * through once asked.
 */
export function MergePullRequestButton(props: {
  target: MergePullRequestTarget;
  status: string | null | undefined;
  draft?: boolean | null;
  checks?: readonly PullRequestCheck[] | null;
  size?: ButtonSize;
  class?: string;
  /** Hide the label and show the icon alone, with the label as its name. */
  iconOnly?: boolean;
  grouped?: boolean;
  onMerged?: () => void;
}) {
  const owner = getOwner();
  const url = () => prHtmlUrl(props.target);
  // Someone else may own this merge (a factory Run's merge Gate): it takes
  // the merge, or says why it doesn't merge now.
  const guard = usePrMergeGuard(() => [url()]);
  const refusal = () => guard.mergeRefusal(url());
  const action = createMergePullRequestAction({
    onMerged: props.onMerged,
    confirm: async (target) => {
      const confirmed = await confirmMergePullRequest(
        { ...target, warning: guard.mergeWarning(url()) },
        owner
      );
      // The guard may have learned the pull request is a Run's while the
      // confirmation was open: it takes the merge then, as before.
      return confirmed && !guard.merge(url());
    },
  });
  const label = () => (action.pending() ? 'Merging…' : 'Merge');
  const Control = props.grouped ? ButtonGroup.Button : Button;
  return (
    <Show
      when={pullRequestReadyToMerge({
        status: props.status,
        draft: props.draft,
        checks: props.checks,
      })}
    >
      <Control
        variant="success"
        size={props.size ?? 'sm'}
        class={cn('shrink-0', props.class)}
        aria-label={`${label()} pull request #${props.target.number}`}
        disabled={action.pending() || refusal() !== undefined}
        tooltip={refusal()}
        data-merge-pull-request={props.target.number}
        on:click={(event) => {
          event.stopPropagation();
          if (guard.merge(url())) return;
          fireAndForget(action.merge(props.target), 'pr-merge');
        }}
      >
        <GitMerge aria-hidden="true" class="size-3.5" />
        <Show when={!props.iconOnly}>
          <span>{label()}</span>
        </Show>
      </Control>
      {guard.gate()}
    </Show>
  );
}
