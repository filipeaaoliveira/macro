/** @vitest-environment jsdom */
import { toast } from '@core/component/Toast/Toast';
import { factoryClient } from '@service-factory/client';
import type { PullRequestRuns } from '@service-factory/generated/schemas';
import { cleanup, render, screen, waitFor } from '@solidjs/testing-library';
import { QueryClient, QueryClientProvider } from '@tanstack/solid-query';
import { err, ok } from 'neverthrow';
import { type Accessor, createSignal, For } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { type FactoryReviews, useFactoryReviews } from './factory-runs';

const flag = vi.hoisted(() => ({ enabled: true, loading: false }));
const viewer = vi.hoisted(() => ({ id: 'macro|julia@example.com' }));

vi.mock('@app/lib/analytics/posthog', () => ({
  useFeatureFlag: () => () => ({
    enabled: flag.enabled,
    loading: flag.loading,
  }),
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
const CHECKING = 'Checking for a factory run…';

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

function mount(urls: Accessor<string[]>, client = queryClient()) {
  let reviews: FactoryReviews | undefined;
  // A component, as the Reviews page is: it reads the factory once.
  function Page() {
    const created = useFactoryReviews(urls);
    reviews = created;
    return (
      <>
        <For each={urls()}>{(url) => created.mark(url)}</For>
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
  return { reviews, client };
}

function queryClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  flag.enabled = true;
  flag.loading = false;
  viewer.id = 'macro|julia@example.com';
});

describe('the factory on the Reviews page', () => {
  it('marks a Run’s pull request and merges it only through the Run’s merge Gate', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(ok(found));
    const { reviews } = mount(() => [COUPON, BANNER]);

    expect(await screen.findByText('Waiting on you to merge')).toBeTruthy();
    expect(factoryClient.findPullRequestRuns).toHaveBeenCalledWith(
      [COUPON, BANNER],
      expect.anything()
    );

    expect(reviews.merge(COUPON)).toBe(true);
    expect(await screen.findByText('Gate of run-1')).toBeTruthy();
    expect(reviews.mergeRefusal(COUPON)).toBeUndefined();
  });

  it('leaves a pull request the factory read and no Run opened to GitHub', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(ok(found));
    const { reviews } = mount(() => [COUPON, OTHER]);
    await screen.findByText('Waiting on you to merge');

    expect(reviews.merge(OTHER)).toBe(false);
    expect(reviews.mergeRefusal(OTHER)).toBeUndefined();
    expect(screen.getAllByText('Factory')).toHaveLength(1);
    expect(toast.failure).not.toHaveBeenCalled();
  });

  it('tells a teammate why they can’t merge, and merges nothing', async () => {
    viewer.id = 'macro|teo@example.com';
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(ok(found));
    const { reviews } = mount(() => [COUPON]);
    await screen.findByText('Factory');

    expect(reviews.merge(COUPON)).toBe(true);
    const why = 'Only its run’s owner can merge this, at its merge gate';
    expect(toast.failure).toHaveBeenCalledWith(why);
    expect(reviews.mergeRefusal(COUPON)).toBe(why);
    expect(screen.queryByText('Gate of run-1')).toBeNull();
  });

  it('merges nothing while the lookup has not answered', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockReturnValue(
      new Promise(() => {})
    );
    const { reviews } = mount(() => [OTHER]);
    await waitFor(() =>
      expect(factoryClient.findPullRequestRuns).toHaveBeenCalled()
    );

    expect(reviews.mergeRefusal(OTHER)).toBe(CHECKING);
    expect(reviews.merge(OTHER)).toBe(true);
    expect(toast.failure).toHaveBeenCalledWith(CHECKING);
  });

  it('merges nothing when the first lookup failed', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(
      err([{ code: 'HTTP_ERROR', message: 'unavailable' }]) as never
    );
    const { reviews, client } = mount(() => [OTHER]);
    await waitFor(() =>
      expect(client.getQueryCache().getAll()[0]?.state.status).toBe('error')
    );

    expect(reviews.merge(OTHER)).toBe(true);
    expect(reviews.mergeRefusal(OTHER)).toBe(CHECKING);
  });

  it('keeps what a lookup read when a read again fails', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValueOnce(
      ok(found)
    );
    const { reviews, client } = mount(() => [COUPON, OTHER]);
    await screen.findByText('Waiting on you to merge');
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(
      err([{ code: 'HTTP_ERROR', message: 'unavailable' }]) as never
    );

    await client.refetchQueries();
    expect(client.getQueryCache().getAll()[0]?.state.status).toBe('error');
    expect(reviews.merge(OTHER)).toBe(false);
    expect(reviews.mergeRefusal(COUPON)).toBeUndefined();
    expect(screen.getByText('Waiting on you to merge')).toBeTruthy();
  });

  it('merges nothing for a pull request the board added until it is read', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValueOnce(
      ok(found)
    );
    const [urls, setUrls] = createSignal([COUPON]);
    const { reviews } = mount(urls);
    await screen.findByText('Waiting on you to merge');
    vi.mocked(factoryClient.findPullRequestRuns).mockReturnValue(
      new Promise(() => {})
    );

    setUrls([COUPON, OTHER]);
    await waitFor(() =>
      expect(factoryClient.findPullRequestRuns).toHaveBeenCalledTimes(2)
    );
    expect(reviews.mergeRefusal(OTHER)).toBe(CHECKING);
    expect(reviews.merge(OTHER)).toBe(true);
    // The pull request read before stays known, and marked.
    expect(screen.getByText('Waiting on you to merge')).toBeTruthy();
  });

  it('merges nothing for a pull request whose Runs the lookup could not all read', async () => {
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(
      ok({ ...found, unknown: [OTHER] })
    );
    const { reviews } = mount(() => [COUPON, OTHER]);
    await screen.findByText('Waiting on you to merge');

    expect(reviews.mergeRefusal(OTHER)).toBe(CHECKING);
    expect(reviews.merge(OTHER)).toBe(true);
  });

  it('merges nothing while the factory flag is not read yet', async () => {
    flag.loading = true;
    const { reviews } = mount(() => [OTHER]);

    expect(reviews.mergeRefusal(OTHER)).toBe(CHECKING);
    expect(reviews.merge(OTHER)).toBe(true);
    expect(factoryClient.findPullRequestRuns).not.toHaveBeenCalled();
  });

  it('asks nothing, and merges as before, while the person doesn’t have the factory', async () => {
    flag.enabled = false;
    const { reviews } = mount(() => [COUPON]);

    await waitFor(() => expect(reviews.merge(COUPON)).toBe(false));
    expect(factoryClient.findPullRequestRuns).not.toHaveBeenCalled();
  });

  it('warns before GitHub merges a pull request whose Run stopped', async () => {
    const listed = found.runs[0]?.listed;
    if (!listed) throw new Error('no run');
    vi.mocked(factoryClient.findPullRequestRuns).mockResolvedValue(
      ok({
        unknown: [],
        runs: [
          {
            url: COUPON,
            mergeGateSeq: null,
            listed: {
              ...listed,
              state: 'stopped',
              detail: {
                ...listed.detail,
                run: {
                  ...listed.detail.run,
                  end: { outcome: 'manual', endedAt: '2026-11-18T10:00:00Z' },
                },
              },
            },
          },
        ],
      })
    );
    const { reviews } = mount(() => [COUPON]);
    await screen.findByText('Factory');

    expect(reviews.merge(COUPON)).toBe(false);
    expect(reviews.mergeWarning(COUPON)).toBe(
      'Its run stopped (Handed off): merging skips its merge gate and the checks it held'
    );
  });
});
