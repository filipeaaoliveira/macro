import { useFeatureFlag } from '@app/lib/analytics/posthog';
import { t } from '@app/lib/i18n';
import type {
  PrMergeGuard,
  PrMergeGuardSource,
} from '@block-pr/primitives/pr-merge-guard-context';
import { toast } from '@core/component/Toast/Toast';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import {
  type Accessor,
  createEffect,
  createMemo,
  createSignal,
  on,
  onCleanup,
  untrack,
} from 'solid-js';

type FactoryModule = typeof import('./factory-runs');

/**
 * The factory's merge guard for the pull requests at `urls`, for the app's
 * pull request views. The factory's code loads only once the person has the
 * factory, outside the startup bundle; until the flag is read and the code
 * has loaded, whether a Run guards a pull request is not known, so nothing
 * merges. Without the factory, GitHub merges every pull request as before.
 */
export const factoryMergeGuard: PrMergeGuardSource = (
  urls: Accessor<readonly string[]>
): PrMergeGuard => {
  const flag = useFeatureFlag(enableFactoryRuns);
  const [factory, setFactory] = createSignal<FactoryModule>();
  let live = true;
  onCleanup(() => {
    live = false;
  });
  createEffect(() => {
    if (flag().loading || !flag().enabled || factory()) return;
    void import('./factory-runs').then((loaded) => {
      if (live) setFactory(() => loaded);
    });
  });
  // The factory's hooks run once its code is here, in this caller's scope,
  // and only then: they read `urls` as they set up, and a memo tracking that
  // would rebuild them, closing an open Gate, whenever the list changed.
  const reviews = createMemo(
    on(factory, (loaded) => untrack(() => loaded?.useFactoryReviews(urls)))
  );
  // Off for this person: no Run guards a pull request.
  const off = () => !flag().loading && !flag().enabled;
  const checking = () => t('factoryBoard.mergeChecking');

  return {
    mark: (url) => <>{reviews()?.mark(url)}</>,
    section: (url) => <>{reviews()?.section(url)}</>,
    merge: (url) => {
      const loaded = reviews();
      if (loaded) return loaded.merge(url);
      if (off()) return false;
      toast.failure(checking());
      return true;
    },
    mergeRefusal: (url) => {
      const loaded = reviews();
      if (loaded) return loaded.mergeRefusal(url);
      return off() ? undefined : checking();
    },
    mergeWarning: (url) => reviews()?.mergeWarning(url),
    gate: () => <>{reviews()?.gate()}</>,
  };
};
