import type { PrActions } from '@block-pr/primitives/create-pr-actions';
import type { Accessor } from 'solid-js';

/** Something that may take a pull request's merge in place of GitHub's. */
export type MergeTaker = {
  /** Takes the merge of the pull request at `url`, returning whether it did. */
  merge: (url: string) => boolean;
  /** Why the pull request at `url` doesn't merge now, if it doesn't. */
  mergeRefusal: (url: string) => string | undefined;
};

/**
 * `actions` with their Merge offered to `taker` first, for the pull request
 * at `url`: disabled with the taker's reason when it refuses one, and
 * GitHub's merge only when the taker doesn't take it. The other actions,
 * and which one leads, stay as they were.
 */
export function withMergeTaker(
  actions: PrActions,
  url: Accessor<string | undefined>,
  taker: MergeTaker
): PrActions {
  const all = () =>
    actions.all().map((action) => {
      const at = url();
      if (action.key !== 'merge' || !at) return action;
      return {
        ...action,
        run: () => {
          if (!taker.merge(at)) action.run();
        },
        disabledReason: taker.mergeRefusal(at) ?? action.disabledReason,
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
