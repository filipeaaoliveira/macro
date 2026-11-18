/** @vitest-environment jsdom */
import type { PrMergeGuard } from '@block-pr/primitives/pr-merge-guard-context';
import { toast } from '@core/component/Toast/Toast';
import { cleanup, render, waitFor } from '@solidjs/testing-library';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { factoryMergeGuard } from './pr-merge-guard';

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({ enabled: true, loading: false }),
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: vi.fn() },
}));
// The factory's code can't be fetched: offline, or a release replaced it.
vi.mock('./factory-runs', () => {
  throw new Error('Failed to fetch dynamically imported module');
});

const URL = 'https://github.com/acme/shop/pull/7';
const FAILED = 'Couldn’t check for a factory run. Reload the page to merge';

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('the factory’s merge guard, when its code doesn’t load', () => {
  it('merges nothing, and says why', async () => {
    let guard: PrMergeGuard | undefined;
    render(() => {
      guard = factoryMergeGuard(() => [URL]);
      return null;
    });
    if (!guard) throw new Error('not mounted');
    const merges = guard;

    await waitFor(() => expect(merges.mergeRefusal(URL)).toBe(FAILED));
    expect(merges.merge(URL)).toBe(true);
    expect(toast.failure).toHaveBeenCalledWith(FAILED);
  });
});
