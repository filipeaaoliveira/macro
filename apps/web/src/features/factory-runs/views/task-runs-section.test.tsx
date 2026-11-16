/** @vitest-environment jsdom */
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@solidjs/testing-library';
import { type Accessor, createSignal, type JSX } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { type RunsContext, RunsProvider } from '../context/runs-context';
import type { Run } from '../core/run';
import { TaskRunsSection } from './task-runs-section';

vi.mock('@components/app/side-panel', () => ({
  useSidePanel: () => null,
  SidePanel: {
    Section: (props: {
      title: JSX.Element;
      actions?: JSX.Element;
      children: JSX.Element;
    }) => (
      <section>
        <h2>{props.title}</h2>
        {props.actions}
        {props.children}
      </section>
    ),
    CountTitle: (props: { label: JSX.Element; count: number }) => (
      <>
        {props.label} ({props.count})
      </>
    ),
  },
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));

vi.stubGlobal(
  'ResizeObserver',
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
);

const running: Run = {
  id: 'run-1',
  request: 'Rename the column',
  createdAt: '2026-11-16T09:00:00Z',
  taskId: 'task-1',
  steps: [{ entry: 'write', visit: 1, startedAt: '2026-11-16T09:00:00Z' }],
};

const started: Run = {
  id: 'run-2',
  request: 'Add a due date column',
  createdAt: '2026-11-16T10:00:00Z',
  taskId: 'task-1',
  steps: [],
};

function mount(
  runs: Run[],
  options: {
    listState?: Accessor<'pending' | 'error' | 'ready'>;
    retry?: () => void;
    startRun?: RunsContext['startRun'];
  } = {}
) {
  const [list, setList] = createSignal(runs);
  const openedRuns: string[] = [];
  const context: RunsContext = {
    taskRuns: () => ({
      state: options.listState ?? (() => 'ready'),
      data: () => (options.listState?.() === 'error' ? undefined : list()),
      retry: options.retry ?? vi.fn(),
    }),
    run: (runId: Accessor<string>) => {
      openedRuns.push(runId());
      return {
        state: () => 'ready',
        data: () => list().find((run) => run.id === runId()),
        retry: vi.fn(),
      };
    },
    flows: () => ({
      state: () => 'ready',
      data: () => [
        { name: 'plan', description: 'Plans the change, then writes it.' },
        { name: 'code', description: 'Takes a task to a merged pull request.' },
      ],
      retry: vi.fn(),
    }),
    startRun:
      options.startRun ??
      vi.fn(async () => {
        setList((current) => [started, ...current]);
        return { ok: true as const, value: started };
      }),
    answerGate: vi.fn(),
    cancelRun: vi.fn(),
  };
  render(() => (
    <RunsProvider value={context}>
      <TaskRunsSection taskId="task-1" order={36} />
    </RunsProvider>
  ));
  return { context, openedRuns };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('TaskRunsSection', () => {
  it('lists the task’s Runs with the Step each one is on', () => {
    mount([running]);

    expect(screen.getByText('Runs (1)')).toBeTruthy();
    expect(screen.getByText('Rename the column')).toBeTruthy();
    expect(screen.getByText('Running Write')).toBeTruthy();
  });

  it('says when the task has no Runs', () => {
    mount([]);

    expect(screen.getByText('No runs yet')).toBeTruthy();
  });

  it('starts a Run of the chosen Flow and opens it', async () => {
    const { context, openedRuns } = mount([running]);

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    const start = await screen.findByRole('button', { name: 'Start run' });
    expect(start.hasAttribute('disabled')).toBe(true);

    fireEvent.input(screen.getByLabelText('Request'), {
      target: { value: '  Add a due date column  ' },
    });
    fireEvent.click(screen.getByRole('radio', { name: /Code/ }));
    fireEvent.click(start);

    await waitFor(() =>
      expect(context.startRun).toHaveBeenCalledWith({
        taskId: 'task-1',
        flow: 'code',
        request: 'Add a due date column',
      })
    );
    await waitFor(() => expect(openedRuns).toEqual(['run-2']));
    expect(screen.queryByRole('button', { name: 'Start run' })).toBeNull();
  });

  it('retries a list that couldn’t be loaded', () => {
    const [state, setState] = createSignal<'pending' | 'error' | 'ready'>(
      'pending'
    );
    const retry = vi.fn(() => setState('ready'));
    mount([running], { listState: state, retry });

    expect(screen.getByRole('status', { name: 'Loading runs…' })).toBeTruthy();
    setState('error');
    expect(screen.getByText('Runs couldn’t be loaded')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(retry).toHaveBeenCalledOnce();
    expect(screen.getByText('Rename the column')).toBeTruthy();
  });

  it('keeps the start dialog open with its error when starting fails', async () => {
    const { openedRuns } = mount([running], {
      startRun: vi.fn(async () => ({
        ok: false as const,
        reason: 'failed' as const,
      })),
    });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    fireEvent.input(await screen.findByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start run' }));

    expect(
      await screen.findByText('Couldn’t start the run. Please try again.')
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Start run' })).toBeTruthy();
    expect(
      (screen.getByLabelText('Request') as HTMLTextAreaElement).value
    ).toBe('Add a due date column');
    expect(openedRuns).toEqual([]);
  });

  it('moves row times on after every Run has ended', () => {
    vi.useFakeTimers({ now: new Date('2026-11-16T09:00:30Z') });
    try {
      mount([
        {
          ...running,
          end: { outcome: 'success', endedAt: '2026-11-16T09:00:20Z' },
        },
      ]);
      expect(screen.getAllByText('now')).not.toHaveLength(0);

      vi.advanceTimersByTime(60_000);
      expect(screen.getAllByText('1m')).not.toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
