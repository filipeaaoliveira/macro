import type { Accessor, Owner } from 'solid-js';
import type { PrForeignEntityData } from '../data/queries';
import { prHtmlUrl } from '../util/prKey';
import type { MergePullRequestTarget } from './create-merge-pull-request-action';
import { createPrActions, type PrActions } from './create-pr-actions';
import { type PrMergeGuard, usePrMergeGuard } from './pr-merge-guard-context';

/**
 * `actions` with their Merge offered to `guard` first, for the pull request
 * at `url`: disabled with the guard's reason when it refuses one, and
 * GitHub's merge only when the guard doesn't take it. The other actions,
 * and which one leads, stay as they were.
 */
export function withMergeGuard(
  actions: PrActions,
  url: Accessor<string | undefined>,
  guard: Pick<PrMergeGuard, 'merge' | 'mergeRefusal'> | undefined
): PrActions {
  if (!guard) return actions;
  const all = () =>
    actions.all().map((action) => {
      const at = url();
      if (action.key !== 'merge' || !at) return action;
      return {
        ...action,
        run: () => {
          if (!guard.merge(at)) action.run();
        },
        disabledReason: guard.mergeRefusal(at) ?? action.disabledReason,
      };
    });
  const primary = () => {
    const key = actions.primary()?.key;
    return all().find((action) => action.key === key);
  };

  return {
    all,
    primary,
    secondary: () => {
      const key = primary()?.key;
      return all().filter((action) => action.key !== key);
    },
    pending: actions.pending,
  };
}

/**
 * A pull request detail's actions with their Merge offered to the app's
 * merge guard first, and the confirmation GitHub's merge asks for carrying
 * the guard's warning; with the guard, for its section and dialogs.
 */
export function createGuardedPrActions(
  detail: Accessor<PrForeignEntityData | undefined>,
  confirmMerge: (
    target: MergePullRequestTarget & { warning?: string },
    owner: Owner | null
  ) => Promise<boolean>
) {
  const url = () => {
    const data = detail();
    return data ? (data.pullRequest.url ?? prHtmlUrl(data.prRef)) : undefined;
  };
  const guard = usePrMergeGuard(() => {
    const at = url();
    return at ? [at] : [];
  });
  const warning = () => {
    const at = url();
    return at ? guard?.mergeWarning(at) : undefined;
  };
  const actions = withMergeGuard(
    createPrActions(detail, (target, owner) =>
      confirmMerge({ ...target, warning: warning() }, owner)
    ),
    url,
    guard
  );
  return { actions, guard, url };
}
