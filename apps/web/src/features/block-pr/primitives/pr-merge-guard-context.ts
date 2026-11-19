import { useFeatureFlag } from '@app/lib/analytics/posthog';
import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { type Accessor, createContext, type JSX, useContext } from 'solid-js';

/**
 * Something that owns some pull requests' merges, such as the factory for
 * the pull requests its Runs opened: what a pull request's views show of
 * it, and the merges it takes in place of GitHub's.
 */
export type PrMergeGuard = {
  /** What a pull request's card shows of it, if anything. */
  mark: (url: string) => JSX.Element;
  /** A section of the pull request's side panel, if any. */
  section: (url: string) => JSX.Element;
  /**
   * Takes the merge of the pull request at `url`, returning whether it did:
   * it then merges, or says why not, its own way. False lets GitHub merge.
   */
  merge: (url: string) => boolean;
  /** Why the pull request at `url` doesn't merge now, if it doesn't. */
  mergeRefusal: (url: string) => string | undefined;
  /** What a person should know before GitHub merges it, if anything. */
  mergeWarning: (url: string) => string | undefined;
  /** What its merges open, such as a dialog. */
  gate: () => JSX.Element;
};

/** Makes the guard for the pull requests at `urls`, in the caller's scope. */
export type PrMergeGuardSource = (
  urls: Accessor<readonly string[]>
) => PrMergeGuard;

const Context = createContext<PrMergeGuardSource>();

/** Provides the app's merge guard to every pull request view under it. */
export const PrMergeGuardProvider = Context.Provider;

/**
 * The guard outside any provider, where no one can say whether a factory
 * Run owns a pull request's merge: for a person who may have the factory
 * (its flag on, or not read yet) it merges nothing, rather than let GitHub
 * merge past a Run's merge Gate; for one without it, GitHub merges as
 * before. A view rendered outside the app's providers is a bug, and this
 * keeps it from becoming a bypass.
 */
function unprovidedMergeGuard(): PrMergeGuard {
  const flag = useFeatureFlag(enableFactoryRuns);
  const off = () => !flag().loading && !flag().enabled;
  const refusal = () => t('factoryBoard.mergeUnguarded');
  return {
    mark: () => null,
    section: () => null,
    merge: () => {
      if (off()) return false;
      toast.failure(refusal());
      return true;
    },
    mergeRefusal: () => (off() ? undefined : refusal()),
    mergeWarning: () => undefined,
    gate: () => null,
  };
}

/**
 * The merge guard for the pull requests at `urls`. Outside a provider, a
 * person who may have the factory merges nothing; one without it merges on
 * GitHub as before.
 */
export function usePrMergeGuard(
  urls: Accessor<readonly string[]>
): PrMergeGuard {
  const source = useContext(Context);
  if (!source) return unprovidedMergeGuard();
  return source(urls);
}
