/** @vitest-environment jsdom */
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from '@solidjs/testing-library';
import { createEffect } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { FactoryBoardView } from './board-view';
import {
  type BoardContext,
  BoardProvider,
  type BoardSource,
} from './context/board-context';
import {
  type RunsContext,
  RunsProvider,
  type RunsQueue,
} from './context/runs-context';
import type { BoardFilters, BoardRun } from './core/board';
import type { Run } from './core/run';

vi.mock('@core/mobile/isTouchDevice', () => ({
  isTouchDevice: () => false,
}));

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

vi.stubGlobal(
  'ResizeObserver',
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
);

const JULIA = 'macro|julia@example.com';
const TEO = 'macro|teo@example.com';

const codeRun: Run = {
  id: 'run-code',
  request: 'Add the coupon field',
  createdAt: '2026-11-18T10:00:00Z',
  ownerId: JULIA,
  flow: 'code',
  createdInPerson: true,
  taskId: 'task-coupon',
  repository: { url: 'https://github.com/acme/shop', baseBranch: 'main' },
  pullRequestUrl: 'https://github.com/acme/shop/pull/42',
  steps: [
    {
      seq: 1,
      entry: 'write',
      visit: 1,
      startedAt: '2026-11-18T10:01:00Z',
      role: 'writer',
      agent: 'primary',
    },
  ],
  reviewRounds: [
    {
      round: 1,
      visit: 1,
      seq: 2,
      reviewers: 1,
      fixDiff: false,
      startedAt: '2026-11-18T10:05:00Z',
      end: { outcome: 'changes', endedAt: '2026-11-18T10:06:00Z' },
      findings: [
        {
          seq: 2,
          place: 1,
          path: 'src/cart.ts',
          problem: 'Rounds the total down',
          why: 'Customers pay less',
          blocksAnywhere: false,
          kind: 'blocker',
        },
      ],
    },
  ],
};

const teosGate: Run = {
  id: 'run-gate',
  request: 'Plan the export',
  createdAt: '2026-11-18T09:00:00Z',
  ownerId: TEO,
  flow: 'plan',
  createdInPerson: false,
  steps: [
    {
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-18T09:00:00Z',
      gate: {
        kind: 'flow',
        question: 'Approve the plan?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-18T09:05:00Z',
      },
    },
  ],
};

const finishedRun: Run = {
  id: 'run-done',
  request: 'Fix the footer',
  createdAt: '2026-11-17T09:00:00Z',
  ownerId: JULIA,
  flow: 'code',
  createdInPerson: true,
  automation: { id: 'automation-1', name: 'Nightly', trigger: 'schedule' },
  end: { outcome: 'success', endedAt: '2026-11-17T10:00:00Z' },
  steps: [],
};

const board: BoardRun[] = [
  {
    id: codeRun.id,
    state: 'running',
    taskName: 'Coupons at checkout',
    run: codeRun,
  },
  { id: teosGate.id, state: 'waiting', run: teosGate },
  { id: finishedRun.id, state: 'finished', run: finishedRun },
];

function mount(options: {
  onOpenRun?: (runId: string) => void;
  runs?: BoardRun[];
  hasMore?: boolean;
  queue?: RunsQueue;
  queueLimit?: number | null;
  cancelRun?: RunsContext['cancelRun'];
}) {
  const asked: BoardFilters[] = [];
  const loadMore = vi.fn();
  const boardContext: BoardContext = {
    board: (filters) => {
      createEffect(() => asked.push(filters()));
      const source: BoardSource = {
        state: () => 'ready',
        data: () => options.runs ?? board,
        retry: vi.fn(),
        hasMore: () => options.hasMore ?? false,
        loadingMore: () => false,
        loadMore,
      };
      return source;
    },
    queueLimit: () => ({
      state: () => 'ready',
      data: () => (options.queueLimit === undefined ? 5 : options.queueLimit),
      retry: vi.fn(),
    }),
    projects: () => ({
      state: () => 'ready',
      data: () => [{ id: 'project-checkout', name: 'Checkout' }],
      retry: vi.fn(),
    }),
    viewerId: () => JULIA,
    personName: (userId) => () => (userId() === TEO ? 'Teo Silva' : undefined),
  };
  const all = (options.runs ?? board).map((listed) => listed.run);
  const runs: RunsContext = {
    viewerId: () => JULIA,
    taskRuns: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => options.queue ?? { waiting: [], starting: [] },
      retry: vi.fn(),
    }),
    run: (runId) => ({
      state: () => 'ready',
      data: () => all.find((run) => run.id === runId()),
      retry: vi.fn(),
    }),
    flows: () => ({
      state: () => 'ready',
      data: () => [
        { name: 'code', description: 'Code', repository: true },
        { name: 'plan', description: 'Plan', repository: false },
      ],
      retry: vi.fn(),
    }),
    repositories: () => ({
      state: () => 'ready',
      data: () => [],
      retry: vi.fn(),
    }),
    startRun: vi.fn(),
    answerGate: vi.fn(),
    replyToGate: vi.fn(),
    cancelRun: options.cancelRun ?? vi.fn(),
    steerStep: vi.fn(),
  };
  render(() => (
    <RunsProvider value={runs}>
      <BoardProvider value={boardContext}>
        <FactoryBoardView onOpenRun={options.onOpenRun} />
      </BoardProvider>
    </RunsProvider>
  ));
  return { asked, loadMore };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe('FactoryBoardView', () => {
  it('groups the runs by where they are, each with its task, place and owner', () => {
    mount({});

    const running = screen.getByRole('region', { name: 'Running' });
    // The task's name stands for the Run; its Step names the Role and Agent.
    expect(within(running).getByText('Coupons at checkout')).toBeTruthy();
    expect(within(running).getByText('Write · writer (primary)')).toBeTruthy();
    expect(
      within(running).getByText('acme/shop · Code · Started by You')
    ).toBeTruthy();
    expect(within(running).getByText('1 blocker')).toBeTruthy();
    expect(
      within(running)
        .getByRole('link', { name: 'Pull request' })
        .getAttribute('href')
    ).toBe('https://github.com/acme/shop/pull/42');

    const waiting = screen.getByRole('region', { name: 'Waiting on a person' });
    expect(within(waiting).getByText('Waiting on Teo Silva')).toBeTruthy();
    expect(
      within(waiting).getByText(
        'Plan · Started by the Orchestrator for Teo Silva'
      )
    ).toBeTruthy();

    const finished = screen.getByRole('region', { name: 'Finished' });
    expect(
      within(finished).getByText('Code · Started by automation Nightly')
    ).toBeTruthy();
    // A merged Run is finished: there is no group after its merge.
    expect(screen.queryByRole('region', { name: 'Held' })).toBeNull();
  });

  it('reads the person’s runs first, then their team’s, a state, and a search', async () => {
    vi.useFakeTimers();
    const { asked } = mount({});
    expect(asked.at(-1)).toEqual({
      scope: 'mine',
      state: undefined,
      flow: undefined,
      repoUrl: undefined,
      projectId: undefined,
      search: undefined,
    });

    fireEvent.click(screen.getByRole('radio', { name: 'My team' }));
    expect(asked.at(-1)?.scope).toBe('team');

    fireEvent.click(screen.getByRole('radio', { name: 'Held' }));
    expect(asked.at(-1)?.state).toBe('held');

    fireEvent.input(
      screen.getByRole('searchbox', { name: 'Search runs by task' }),
      {
        target: { value: '  coupon ' },
      }
    );
    // The search reads once typing pauses.
    expect(asked.at(-1)?.search).toBeUndefined();
    vi.advanceTimersByTime(300);
    expect(asked.at(-1)?.search).toBe('coupon');
  });

  it('shows the person’s queue against its limit and the runs waiting on them', () => {
    mount({
      queue: { waiting: [teosGate], starting: [codeRun] },
      queueLimit: 5,
    });

    expect(screen.getByText('2 of 5 places')).toBeTruthy();
    expect(document.querySelector('[data-board-waiting]')?.textContent).toBe(
      '1'
    );
  });

  it('says a queue has no limit', () => {
    mount({ queueLimit: null });

    expect(screen.getByText('0 places, no limit')).toBeTruthy();
  });

  it('reads the next page on Show more', () => {
    const { loadMore } = mount({ hasMore: true });

    fireEvent.click(screen.getByRole('button', { name: 'Show more' }));

    expect(loadMore).toHaveBeenCalledOnce();
  });

  it('opens a run from its row, with its owner’s controls only for its owner', async () => {
    mount({});

    fireEvent.click(
      screen.getByRole('button', { name: /Coupons at checkout/ })
    );
    expect(
      await screen.findByRole('button', { name: 'Cancel run' })
    ).toBeTruthy();
    cleanup();

    mount({});
    fireEvent.click(screen.getByRole('button', { name: /Plan the export/ }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('Plan the export')).toBeTruthy();
    // A team member's Run reads without its Gate's answers or Cancel run.
    expect(screen.queryByRole('button', { name: 'Cancel run' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull();
  });

  it('opens a run in a pane of its own where the app gives one', () => {
    const onOpenRun = vi.fn();
    mount({ onOpenRun });

    fireEvent.click(
      screen.getByRole('button', { name: /Coupons at checkout/ })
    );

    expect(onOpenRun).toHaveBeenCalledWith('run-code');
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('says when no run is there', () => {
    mount({ runs: [] });

    expect(screen.getByText('No runs here')).toBeTruthy();
  });
});
