import { useFeatureFlag } from '@app/lib/analytics/posthog';
import { t } from '@app/lib/i18n';
import type {
  PrMergeGuard,
  PrMergeGuardSource,
} from '@block-pr/primitives/pr-merge-guard-context';
import { toast } from '@core/component/Toast/Toast';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { fireAndForget } from '@core/util/fireAndForget';
import { ResultAsync } from 'neverthrow';
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
  // The factory's code could not be loaded (offline, or a release replaced
  // its chunk): nothing merges still, and the guard says why.
  const [failed, setFailed] = createSignal(false);
  let live = true;
  onCleanup(() => {
    live = false;
  });
  const load = async () => {
    const loaded = await ResultAsync.fromPromise(
      import('./factory-runs'),
      (error) => error
    );
    if (!live) return;
    if (loaded.isErr()) {
      setFailed(true);
      return;
    }
    setFactory(() => loaded.value);
  };
  createEffect(() => {
    if (flag().loading || !flag().enabled || factory() || failed()) return;
    fireAndForget(load(), 'factory-runs');
  });
  // The factory's hooks run once its code is here, in this caller's scope,
  // and only then: they read `urls` as they set up, and a memo tracking that
  // would rebuild them, closing an open Gate, whenever the list changed.
  const reviews = createMemo(
    on(factory, (loaded) => untrack(() => loaded?.useFactoryReviews(urls)))
  );
  // Off for this person: no Run guards a pull request.
  const off = () => !flag().loading && !flag().enabled;
  const checking = () =>
    failed()
      ? t('factoryBoard.mergeLoadFailed')
      : t('factoryBoard.mergeChecking');

  return {
    mark: (url) => <>{reviews()?.mark(url)}</>,
    section: (url) => <>{reviews()?.section(url)}</>,
    merge: (url) => {
      const loaded = reviews();
      if (loaded) return loaded.merge(url);
      if (off()) return false;
      toast.failure(checking());
      // Asked to merge after the load failed: try loading again.
      setFailed(false);
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
