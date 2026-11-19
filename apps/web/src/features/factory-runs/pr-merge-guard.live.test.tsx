/** @vitest-environment jsdom */
import type { PrMergeGuard } from '@block-pr/primitives/pr-merge-guard-context';
import { factoryClient } from '@service-factory/client';
import type { PullRequestRuns } from '@service-factory/generated/schemas';
import { cleanup, render, screen, waitFor } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { createSignal } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { factoryMergeGuard } from './pr-merge-guard';

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({ enabled: true, loading: false }),
  ShowFeatureFlag: (props: { children: unknown }) => props.children,
}));
vi.mock('@service-factory/client', () => ({
  factoryClient: { findPullRequestRuns: vi.fn() },
}));
vi.mock('@core/context/user', () => ({
  useUserId: () => () => 'macro|julia@example.com',
}));
vi.mock('@queries/auth/user-names', () => ({
  normalizeUserNameQueryId: (id: string) => id,
  useUserNames: () => () => [],
}));
vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));
vi.mock('./run-view', () => ({
  RunView: (props: { runId: string }) => <p>Gate of {props.runId}</p>,
  RunPageView: () => null,
}));

const COUPON = 'https://github.com/acme/shop/pull/7';
const OTHER = 'https://github.com/acme/shop/pull/9';

/** Julia's Run on the coupon pull request, waiting at its merge Gate. */
const found: PullRequestRuns = {
  unknown: [],
  runs: [
    {
      url: COUPON,
      mergeGateSeq: 4,
      listed: {
        state: 'waiting',
        detail: {
          run: {
            id: 'run-1',
            request: 'Add the coupon field',
            createdAt: '2026-11-18T09:00:00Z',
            userId: 'macro|julia@example.com',
            createdInPerson: true,
            pullRequestUrl: COUPON,
          },
          steps: [
            {
              seq: 4,
              entry: 'merge-gate',
              visit: 1,
              startedAt: '2026-11-18T09:30:00Z',
              checks: [],
              passedOver: [],
              terminal: { betweenTurns: false, steers: [] },
            },
          ],
          reviewRounds: [],
          standingActs: [],
        },
      },
    },
  ],
};

// The guard loads the factory's code on demand; load it once up front, as a
// person who had opened the factory would have it.
beforeAll(async () => {
  await import('./factory-runs');
}, 60_000);

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('the factory’s merge guard, with the factory’s own lookup', () => {
  it('keeps an open merge Gate and what it knew when the pull requests change', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValueOnce(
      ok(found)
    );
    const [urls, setUrls] = createSignal<readonly string[]>([COUPON]);
    let guard: PrMergeGuard | undefined;
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    // A component, as a pull request view is: it makes the guard once.
    function Page() {
      const made = factoryMergeGuard(urls);
      guard = made;
      return <>{made.gate()}</>;
    }
    render(() => (
      <QueryClientProvider client={client}>
        <Page />
      </QueryClientProvider>
    ));
    if (!guard) throw new Error('not mounted');
    const merges = guard;
    await waitFor(() => expect(merges.mergeRefusal(COUPON)).toBeUndefined());
    expect(merges.merge(COUPON)).toBe(true);
    expect(await screen.findByText('Gate of run-1')).toBeTruthy();
    vi.mocked(factoryClient.findPullRequestRuns).mockReturnValue(
      new Promise(() => {})
    );

    setUrls([COUPON, OTHER]);
    await waitFor(() =>
      expect(factoryClient.findPullRequestRuns).toHaveBeenCalledWith(
        [COUPON, OTHER],
        expect.anything()
      )
    );

    expect(screen.getByText('Gate of run-1')).toBeTruthy();
    expect(merges.mergeRefusal(COUPON)).toBeUndefined();
    expect(merges.mergeRefusal(OTHER)).toBe('Checking for a factory run…');
  });
});
