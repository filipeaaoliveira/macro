/** @vitest-environment jsdom */
import { toast } from '@core/component/Toast/Toast';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@solidjs/testing-library';
import { type Accessor, createSignal } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
  type RetryRunResult,
  type RunsContext,
  RunsProvider,
} from './context/runs-context';
import type { Run } from './core/run';
import { RunPageView, RunView } from './run-view';

const touch = vi.hoisted(() => ({ device: false }));

vi.mock('@core/mobile/isTouchDevice', () => ({
  isTouchDevice: () => touch.device,
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
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

const OWNER = 'macro|julia@example.com';

const failedRun: Run = {
  id: 'run-1',
  request: 'Add a due date column',
  ownerId: OWNER,
  createdAt: '2026-11-16T09:00:00Z',
  taskId: 'task-1',
  flow: 'code',
  end: { outcome: 'infrastructure', endedAt: '2026-11-16T10:00:00Z' },
  steps: [
    {
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-16T09:00:00Z',
      end: {
        outcome: 'done',
        output: 'Add a `due_date` column.',
        endedAt: '2026-11-16T09:01:00Z',
      },
    },
  ],
};

const retryRun: Run = {
  id: 'run-2',
  request: 'Add a due date column',
  ownerId: OWNER,
  createdAt: '2026-11-16T10:05:00Z',
  taskId: 'task-1',
  flow: 'code',
  retry: { retriedFrom: 'run-1', branchOf: 'run-1', firstSeq: 2 },
  steps: failedRun.steps,
};

function mount(options: {
  runs?: Run[];
  taskRuns?: Run[];
  viewerId?: string;
  retryRun?: RunsContext['retryRun'];
  view?: 'dialog' | 'page';
  onOpenRun?: (runId: string) => void;
}) {
  const [runs] = createSignal(options.runs ?? [failedRun, retryRun]);
  const byId = (runId: Accessor<string>) => ({
    state: () => 'ready' as const,
    data: () => runs().find((run) => run.id === runId()),
    retry: vi.fn(),
  });
  const taskRuns = vi.fn((taskId: Accessor<string>) => ({
    state: () => 'ready' as const,
    data: () => (taskId() ? (options.taskRuns ?? []) : undefined),
    retry: vi.fn(),
  }));
  const context: RunsContext = {
    viewerId: () => options.viewerId ?? OWNER,
    taskRuns,
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => ({ waiting: [], starting: [] }),
      retry: vi.fn(),
    }),
    run: byId,
    flows: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    repositories: () => ({
      state: () => 'ready',
      data: () => [],
      retry: vi.fn(),
    }),
    startRun: vi.fn(),
    answerGate: vi.fn(),
    replyToGate: vi.fn(),
    cancelRun: vi.fn(),
    retryRun:
      options.retryRun ??
      vi.fn(
        async (): Promise<RetryRunResult> => ({
          ok: true,
          value: retryRun,
        })
      ),
    markRunFlawed: vi.fn(),
    runScores: () => ({
      state: () => 'ready',
      data: () => [],
      retry: vi.fn(),
    }),
    labelRunScore: vi.fn(),
    steerStep: vi.fn(),
  };
  render(() => (
    <RunsProvider value={context}>
      {options.view === 'page' ? (
        <RunPageView runId="run-1" onOpenRun={options.onOpenRun} />
      ) : (
        <RunView runId="run-1" onClose={vi.fn()} />
      )}
    </RunsProvider>
  ));
  return { context, taskRuns };
}

const retryButton = () => screen.queryByRole('button', { name: 'Retry run' });

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  touch.device = false;
});

describe('retrying a Run', () => {
  it('retries the owner’s failed Run and opens its retry in its place', async () => {
    const { context } = mount({});

    const button = await screen.findByRole('button', { name: 'Retry run' });
    fireEvent.click(button);

    await waitFor(() => expect(context.retryRun).toHaveBeenCalledWith('run-1'));
    expect(
      await screen.findByRole('button', { name: 'Open the run it retries' })
    ).toBeTruthy();
    expect(retryButton()).toBeNull();
  });

  it('opens the retry where the page view says, in the pane’s place', async () => {
    const onOpenRun = vi.fn();
    mount({ view: 'page', onOpenRun });

    fireEvent.click(await screen.findByRole('button', { name: 'Retry run' }));

    await waitFor(() => expect(onOpenRun).toHaveBeenCalledWith('run-2'));
  });

  it('offers a retry on the phone sheet too', async () => {
    touch.device = true;
    mount({});

    expect(
      await screen.findByRole('button', { name: 'Retry run' })
    ).toBeTruthy();
  });

  it('offers no retry to a team member reading someone else’s Run', async () => {
    const { taskRuns } = mount({ viewerId: 'macro|ana@example.com' });

    expect(await screen.findByText('Add a due date column')).toBeTruthy();
    expect(retryButton()).toBeNull();
    // A team member's list holds only their own Runs: it is not read.
    expect(taskRuns.mock.calls.some(([taskId]) => taskId() !== '')).toBe(false);
  });

  it('offers no retry for a Run that ended other than failed, infrastructure or blocked', async () => {
    for (const outcome of ['success', 'cancelled', 'manual'] as const) {
      mount({
        runs: [
          {
            ...failedRun,
            end: { outcome, endedAt: '2026-11-16T10:00:00Z' },
          },
        ],
      });
      expect(await screen.findByText('Add a due date column')).toBeTruthy();
      expect(retryButton()).toBeNull();
      cleanup();
    }
    mount({ runs: [{ ...failedRun, end: undefined }] });
    expect(await screen.findByText('Add a due date column')).toBeTruthy();
    expect(retryButton()).toBeNull();
  });

  it('offers no retry for a benchmark trial', async () => {
    mount({ runs: [{ ...failedRun, flow: 'benchmark' }] });

    expect(await screen.findByText('Add a due date column')).toBeTruthy();
    expect(retryButton()).toBeNull();
  });

  it('links a retried Run to its retry from its task’s Runs, and offers no second retry', async () => {
    mount({ taskRuns: [retryRun, failedRun] });

    fireEvent.click(
      await screen.findByRole('button', { name: 'Open its retry' })
    );

    expect(
      await screen.findByRole('button', { name: 'Open the run it retries' })
    ).toBeTruthy();
    expect(retryButton()).toBeNull();
  });

  it('says why a retry was refused, and offers it again', async () => {
    const results: RetryRunResult[] = [
      { ok: false, reason: 'queueFull' },
      { ok: false, reason: 'notInStandingRole' },
      { ok: false, reason: 'definitionUnreadable' },
      { ok: false, reason: 'failed' },
    ];
    const retry = vi.fn(async () => results.shift() ?? results[0]);
    mount({ retryRun: retry as RunsContext['retryRun'] });
    const button = await screen.findByRole('button', { name: 'Retry run' });

    fireEvent.click(button);
    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith(
        expect.stringContaining('Your queue is full')
      )
    );
    fireEvent.click(button);
    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith(
        expect.stringContaining('no standing role here')
      )
    );
    fireEvent.click(button);
    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith(
        expect.stringContaining('factory definition doesn’t read')
      )
    );
    fireEvent.click(button);
    await waitFor(() =>
      expect(toast.failure).toHaveBeenCalledWith('Couldn’t retry the run')
    );
    // These refusals leave the Run where it is, to retry later.
    expect(retryButton()).toBeTruthy();
  });

  it('stops offering a retry the factory refused as not retriable', async () => {
    const retry = vi.fn(
      async (): Promise<RetryRunResult> => ({ ok: false, reason: 'conflict' })
    );
    mount({ retryRun: retry });

    fireEvent.click(await screen.findByRole('button', { name: 'Retry run' }));

    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith(
        'This run can’t be retried',
        expect.anything()
      )
    );
    await waitFor(() => expect(retryButton()).toBeNull());
  });

  it('shows a linked Run without the note written on the one before', async () => {
    const earlier: Run = { ...failedRun, id: 'run-0' };
    const waitingRetry: Run = {
      ...retryRun,
      id: 'run-1',
      retry: { retriedFrom: 'run-0', branchOf: 'run-0', firstSeq: 2 },
      steps: [
        {
          ...failedRun.steps[0],
          seq: 2,
          gate: {
            kind: 'flow',
            question: 'Approve this plan?',
            answers: ['approve', 'revise'],
            openedAt: '2026-11-16T10:06:00Z',
          },
        } as Run['steps'][number],
      ],
    };
    mount({ runs: [earlier, waitingRetry], taskRuns: [waitingRetry, earlier] });

    const note = await screen.findByRole('textbox');
    fireEvent.input(note, { target: { value: 'Keep it small' } });
    fireEvent.click(
      screen.getByRole('button', { name: 'Open the run it retries' })
    );
    fireEvent.click(
      await screen.findByRole('button', { name: 'Open its retry' })
    );

    expect(
      ((await screen.findByRole('textbox')) as HTMLTextAreaElement).value
    ).toBe('');
  });
});
