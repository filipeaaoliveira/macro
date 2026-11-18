/** @vitest-environment jsdom */
import { toast } from '@core/component/Toast/Toast';
import { factoryClient } from '@service-factory/client';
import type { PullRequestRuns } from '@service-factory/generated/schemas';
import { cleanup, render, screen, waitFor } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { ok } from 'neverthrow';
import { For } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { type FactoryReviews, useFactoryReviews } from './factory-runs';

const flag = vi.hoisted(() => ({ enabled: true }));
const viewer = vi.hoisted(() => ({ id: 'macro|julia@example.com' }));

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({ enabled: flag.enabled, loading: false }),
  ShowFeatureFlag: (props: { children: unknown }) => props.children,
}));
vi.mock('@service-factory/client', () => ({
  factoryClient: { findPullRequestRuns: vi.fn() },
}));
vi.mock('@core/context/user', () => ({
  useUserId: () => () => viewer.id,
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
const BANNER = 'https://github.com/acme/shop/pull/8';
const OTHER = 'https://github.com/acme/shop/pull/9';

/** Julia's Run on the coupon pull request, waiting at its merge Gate. */
const found: PullRequestRuns = {
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
            pullRequestUrl: 'https://github.com/Acme/Shop/pull/7',
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
        },
      },
    },
  ],
};

function mount(urls: string[]): FactoryReviews {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  let reviews: FactoryReviews | undefined;
  // A component, as the Reviews page is: it reads the factory once.
  function Page() {
    const created = useFactoryReviews(() => urls);
    reviews = created;
    return (
      <>
        <For each={urls}>{(url) => created.mark(url)}</For>
        {created.gate()}
      </>
    );
  }
  render(() => (
    <QueryClientProvider client={client}>
      <Page />
    </QueryClientProvider>
  ));
  if (!reviews) throw new Error('not mounted');
  return reviews;
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  flag.enabled = true;
  viewer.id = 'macro|julia@example.com';
});

describe('the factory on the Reviews page', () => {
  it('marks a Run’s pull request and merges it only through the Run’s merge Gate', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(ok(found));
    const reviews = mount([COUPON, BANNER]);

    expect(await screen.findByText('Waiting on you to merge')).toBeTruthy();
    expect(factoryClient.findPullRequestRuns).toHaveBeenCalledWith(
      [COUPON, BANNER],
      expect.anything()
    );

    expect(reviews.merge(COUPON)).toBe(true);
    expect(await screen.findByText('Gate of run-1')).toBeTruthy();
    expect(reviews.mergeRefusal(COUPON)).toBeUndefined();
  });

  it('leaves a pull request no Run opened to GitHub, unmarked', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(ok(found));
    const reviews = mount([COUPON, OTHER]);
    await screen.findByText('Waiting on you to merge');

    expect(reviews.merge(OTHER)).toBe(false);
    expect(reviews.mergeRefusal(OTHER)).toBeUndefined();
    expect(screen.getAllByText('Factory')).toHaveLength(1);
    expect(toast.failure).not.toHaveBeenCalled();
  });

  it('tells a teammate why they can’t merge, and merges nothing', async () => {
    viewer.id = 'macro|teo@example.com';
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(ok(found));
    const reviews = mount([COUPON]);
    await screen.findByText('Factory');

    expect(reviews.merge(COUPON)).toBe(true);
    const why = 'Only its run’s owner can merge this, at its merge gate';
    expect(toast.failure).toHaveBeenCalledWith(why);
    expect(reviews.mergeRefusal(COUPON)).toBe(why);
    expect(screen.queryByText('Gate of run-1')).toBeNull();
  });

  it('asks nothing while the person doesn’t have the factory', async () => {
    flag.enabled = false;
    const reviews = mount([COUPON]);

    await waitFor(() => expect(reviews.merge(COUPON)).toBe(false));
    expect(factoryClient.findPullRequestRuns).not.toHaveBeenCalled();
  });
});
