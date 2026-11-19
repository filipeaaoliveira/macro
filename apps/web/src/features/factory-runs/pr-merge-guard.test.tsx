/** @vitest-environment jsdom */
import type { PrMergeGuard } from '@block-pr/primitives/pr-merge-guard-context';
import { toast } from '@core/component/Toast/Toast';
import { cleanup, render, waitFor } from '@solidjs/testing-library';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { factoryMergeGuard } from './pr-merge-guard';

const flag = vi.hoisted(() => ({ enabled: true, loading: false }));
const factory = vi.hoisted(() => ({ merge: vi.fn(() => true) }));

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({
    enabled: flag.enabled,
    loading: flag.loading,
  }),
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { failure: vi.fn() },
}));
vi.mock('./factory-runs', () => ({
  useFactoryReviews: () => ({
    mark: () => null,
    section: () => null,
    merge: factory.merge,
    mergeRefusal: () => 'Only Julia can merge this',
    mergeWarning: () => undefined,
    gate: () => null,
  }),
}));

const URL = 'https://github.com/acme/shop/pull/7';
const CHECKING = 'Checking for a factory run…';

function mount(): PrMergeGuard {
  let guard: PrMergeGuard | undefined;
  render(() => {
    guard = factoryMergeGuard(() => [URL]);
    return null;
  });
  if (!guard) throw new Error('not mounted');
  return guard;
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  flag.enabled = true;
  flag.loading = false;
});

describe('the factory’s merge guard', () => {
  it('merges nothing while the factory flag is not read yet', () => {
    flag.loading = true;
    const guard = mount();

    expect(guard.mergeRefusal(URL)).toBe(CHECKING);
    expect(guard.merge(URL)).toBe(true);
    expect(toast.failure).toHaveBeenCalledWith(CHECKING);
  });

  it('lets GitHub merge for a person without the factory', () => {
    flag.enabled = false;
    const guard = mount();

    expect(guard.mergeRefusal(URL)).toBeUndefined();
    expect(guard.merge(URL)).toBe(false);
  });

  it('merges nothing until the factory’s code is here, then asks it', async () => {
    const guard = mount();
    expect(guard.mergeRefusal(URL)).toBe(CHECKING);

    await waitFor(() =>
      expect(guard.mergeRefusal(URL)).toBe('Only Julia can merge this')
    );
    expect(guard.merge(URL)).toBe(true);
    expect(factory.merge).toHaveBeenCalledWith(URL);
  });
});
