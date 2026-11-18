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

const device = vi.hoisted(() => ({
  touch: false,
  setPanelOpen: vi.fn<(open: boolean) => void>(),
}));

vi.mock('@core/mobile/isTouchDevice', () => ({
  isTouchDevice: () => device.touch,
}));

vi.mock('@components/app/side-panel', () => ({
  useSidePanel: () => ({ setIsOpen: device.setPanelOpen }),
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
  ownerId: 'macro|julia@example.com',
  createdAt: '2026-11-16T09:00:00Z',
  taskId: 'task-1',
  steps: [
    { seq: 1, entry: 'write', visit: 1, startedAt: '2026-11-16T09:00:00Z' },
  ],
};

const started: Run = {
  id: 'run-2',
  request: 'Add a due date column',
  ownerId: 'macro|julia@example.com',
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
    flowsState?: Accessor<'pending' | 'error' | 'ready'>;
    flowsRetry?: () => void;
    repositories?: Accessor<string[]>;
    onOpenInsights?: () => void;
  } = {}
) {
  const [list, setList] = createSignal(runs);
  const openedRuns: string[] = [];
  const context: RunsContext = {
    viewerId: () => 'macro|julia@example.com',
    taskRuns: () => ({
      state: options.listState ?? (() => 'ready'),
      data: () => (options.listState?.() === 'error' ? undefined : list()),
      retry: options.retry ?? vi.fn(),
    }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => ({ waiting: [], starting: [] }),
      retry: vi.fn(),
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
      state: options.flowsState ?? (() => 'ready'),
      data: () =>
        options.flowsState && options.flowsState() !== 'ready'
          ? undefined
          : [
              // In the order the service lists them.
              {
                name: 'code',
                description: 'Takes a task to a merged pull request.',
                repository: true,
              },
              {
                name: 'plan',
                description: 'Plans the change, then writes it.',
                repository: false,
              },
              {
                name: 'reply',
                description: 'Answers the request.',
                repository: false,
              },
            ],
      retry: options.flowsRetry ?? vi.fn(),
    }),
    repositories: () => ({
      state: () => 'ready',
      data:
        options.repositories ??
        (() => [
          'https://github.com/macro-inc/app',
          'https://github.com/macro-inc/docs',
        ]),
      retry: vi.fn(),
    }),
    startRun:
      options.startRun ??
      vi.fn(async () => {
        setList((current) => [started, ...current]);
        return { ok: true as const, value: started };
      }),
    answerGate: vi.fn(),
    replyToGate: vi.fn(),
    cancelRun: vi.fn(),
    markRunFlawed: vi.fn(),
    steerStep: vi.fn(),
  };
  render(() => (
    <RunsProvider value={context}>
      <TaskRunsSection
        taskId="task-1"
        order={36}
        onOpenInsights={options.onOpenInsights}
      />
    </RunsProvider>
  ));
  return { context, openedRuns };
}

afterEach(() => {
  device.touch = false;
  cleanup();
  vi.clearAllMocks();
});

describe('TaskRunsSection', () => {
  it('lists the task’s Runs with the Step each one is on', () => {
    mount([running]);

    expect(screen.getByText('Runs (1)')).toBeTruthy();
    expect(screen.getByText('Rename the column')).toBeTruthy();
    expect(screen.getByText('Running write')).toBeTruthy();
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
    expect(screen.getByRole('radiogroup', { name: 'Flow' })).toBeTruthy();
    fireEvent.click(screen.getByRole('radio', { name: /Reply/ }));
    fireEvent.click(start);

    await waitFor(() =>
      expect(context.startRun).toHaveBeenCalledWith({
        taskId: 'task-1',
        flow: 'reply',
        request: 'Add a due date column',
      })
    );
    await waitFor(() => expect(openedRuns).toEqual(['run-2']));
    expect(screen.queryByRole('button', { name: 'Start run' })).toBeNull();
  });

  it('starts a Run of a Flow that works on a repository on the one chosen', async () => {
    const { context } = mount([running]);

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    const start = await screen.findByRole('button', { name: 'Start run' });
    fireEvent.input(screen.getByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    expect(screen.queryByRole('combobox', { name: 'Repository' })).toBeNull();
    fireEvent.click(screen.getByRole('radio', { name: /Code/ }));
    // It doesn't start until a repository is chosen.
    expect(start.hasAttribute('disabled')).toBe(true);

    const repository = screen.getByRole('combobox', { name: 'Repository' });
    fireEvent.input(repository, { target: { value: 'docs' } });
    fireEvent.click(
      await screen.findByRole('option', { name: 'macro-inc/docs' })
    );
    expect(start.hasAttribute('disabled')).toBe(false);
    fireEvent.click(start);

    await waitFor(() =>
      expect(context.startRun).toHaveBeenCalledWith({
        taskId: 'task-1',
        flow: 'code',
        request: 'Add a due date column',
        repoUrl: 'https://github.com/macro-inc/docs',
      })
    );
  });

  it('opens on the first Flow that needs no repository', async () => {
    mount([running]);

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    await screen.findByRole('radiogroup', { name: 'Flow' });

    expect(
      (screen.getByRole('radio', { name: /Plan/ }) as HTMLInputElement).checked
    ).toBe(true);
    expect(screen.queryByRole('combobox', { name: 'Repository' })).toBeNull();
  });

  it('starts a Flow that needs no repository with none, though one was chosen before', async () => {
    const { context } = mount([running]);

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    const start = await screen.findByRole('button', { name: 'Start run' });
    fireEvent.input(screen.getByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('radio', { name: /Code/ }));
    fireEvent.input(screen.getByRole('combobox', { name: 'Repository' }), {
      target: { value: 'docs' },
    });
    fireEvent.click(
      await screen.findByRole('option', { name: 'macro-inc/docs' })
    );
    fireEvent.click(screen.getByRole('radio', { name: /Reply/ }));
    fireEvent.click(start);

    await waitFor(() => expect(context.startRun).toHaveBeenCalledOnce());
    const [input] = vi.mocked(context.startRun).mock.calls[0] ?? [];
    expect(input).not.toHaveProperty('repoUrl');
    expect(input).toEqual({
      taskId: 'task-1',
      flow: 'reply',
      request: 'Add a due date column',
    });
  });

  it('lets go of a repository a later read no longer lists', async () => {
    const [listed, setListed] = createSignal([
      'https://github.com/macro-inc/app',
      'https://github.com/macro-inc/docs',
    ]);
    mount([running], { repositories: listed });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    const start = await screen.findByRole('button', { name: 'Start run' });
    fireEvent.input(screen.getByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('radio', { name: /Code/ }));
    fireEvent.input(screen.getByRole('combobox', { name: 'Repository' }), {
      target: { value: 'docs' },
    });
    fireEvent.click(
      await screen.findByRole('option', { name: 'macro-inc/docs' })
    );
    expect(start.hasAttribute('disabled')).toBe(false);

    setListed(['https://github.com/macro-inc/app']);

    expect(start.hasAttribute('disabled')).toBe(true);
  });

  it('says why a repository was refused, without asking to try again', async () => {
    mount([running], {
      startRun: vi.fn(async () => ({
        ok: false as const,
        reason: 'noPushAccess' as const,
      })),
    });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    const start = await screen.findByRole('button', { name: 'Start run' });
    fireEvent.input(screen.getByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('radio', { name: /Code/ }));
    fireEvent.input(screen.getByRole('combobox', { name: 'Repository' }), {
      target: { value: 'docs' },
    });
    fireEvent.click(
      await screen.findByRole('option', { name: 'macro-inc/docs' })
    );
    fireEvent.click(start);

    expect(
      await screen.findByText(
        'Your linked GitHub account can’t push to this repository. Choose one it can push to.'
      )
    ).toBeTruthy();
    expect(screen.queryByText(/Please try again/)).toBeNull();
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

  it('sends a start refused for a full queue to Waiting on you', async () => {
    const onOpenInsights = vi.fn();
    mount([running], {
      startRun: vi.fn(async () => ({
        ok: false as const,
        reason: 'queueFull' as const,
      })),
      onOpenInsights,
    });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    fireEvent.input(await screen.findByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start run' }));

    expect((await screen.findByRole('alert')).textContent).toContain(
      'Your queue is full'
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'Open Waiting on you' })
    );

    expect(onOpenInsights).toHaveBeenCalledOnce();
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Start run' })).toBeNull()
    );
  });

  it('offers no way to Waiting on you for another refusal', async () => {
    mount([running], {
      startRun: vi.fn(async () => ({
        ok: false as const,
        reason: 'failed' as const,
      })),
      onOpenInsights: vi.fn(),
    });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    fireEvent.input(await screen.findByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start run' }));

    expect(await screen.findByRole('alert')).toBeTruthy();
    expect(
      screen.queryByRole('button', { name: 'Open Waiting on you' })
    ).toBeNull();
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

  it('retries Flows that couldn’t be loaded', async () => {
    const [flowsState, setFlowsState] = createSignal<
      'pending' | 'error' | 'ready'
    >('error');
    const flowsRetry = vi.fn(() => setFlowsState('ready'));
    mount([running], { flowsState, flowsRetry });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    expect(await screen.findByText('Flows couldn’t be loaded')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(flowsRetry).toHaveBeenCalledOnce();
    expect(screen.getByRole('radiogroup', { name: 'Flow' })).toBeTruthy();
  });

  it('keeps the start dialog open while a start is on the wire', async () => {
    let finish!: () => void;
    mount([running], {
      startRun: vi.fn(
        () =>
          new Promise<{ ok: false; reason: 'failed' }>((resolve) => {
            finish = () => resolve({ ok: false, reason: 'failed' });
          })
      ),
    });

    fireEvent.click(screen.getByRole('button', { name: 'New run' }));
    fireEvent.input(await screen.findByLabelText('Request'), {
      target: { value: 'Add a due date column' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start run' }));
    fireEvent.keyDown(screen.getByLabelText('Request'), { key: 'Escape' });

    expect(screen.getByRole('button', { name: 'Start run' })).toBeTruthy();
    expect(
      screen.getByRole('button', { name: 'Cancel' }).hasAttribute('disabled')
    ).toBe(true);
    finish();
    expect(
      await screen.findByText('Couldn’t start the run. Please try again.')
    ).toBeTruthy();
  });

  it('closes the side panel’s sheet on touch before a Run opens', () => {
    device.touch = true;
    mount([running]);

    fireEvent.click(screen.getByText('Rename the column'));

    expect(device.setPanelOpen).toHaveBeenCalledWith(false);
  });
});
