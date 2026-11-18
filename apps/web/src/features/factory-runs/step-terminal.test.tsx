/** @vitest-environment jsdom */
import { toast } from '@core/component/Toast/Toast';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@solidjs/testing-library';
import { createSignal, type ParentProps } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
  type RunActionResult,
  type RunsContext,
  RunsProvider,
  type SteerStepInput,
} from './context/runs-context';
import type { Run, RunStep } from './core/run';
import { RunView } from './run-view';

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));

// The Step Terminal shows the session as Macro's agent session view does:
// the same provider and transcript, which stand in here for the live ones.
vi.mock('@app/features/block-agent/agent-session-provider', () => ({
  AgentSessionProvider: (props: ParentProps<{ blockId: string }>) => (
    <div data-agent-session={props.blockId}>{props.children}</div>
  ),
}));
vi.mock('@app/features/block-agent/component/Transcript', () => ({
  Transcript: (props: { replies?: boolean }) => (
    <p data-replies={String(props.replies)}>The agent’s transcript</p>
  ),
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

const writing: RunStep = {
  seq: 2,
  entry: 'write',
  visit: 1,
  startedAt: '2026-11-16T09:01:00Z',
  role: 'writer',
  agent: 'primary',
  terminal: { sessionId: 'session-2', steers: [] },
};

const runningRun: Run = {
  id: 'run-1',
  request: 'Add a due date column',
  createdAt: '2026-11-16T09:00:00Z',
  taskId: 'task-1',
  steps: [
    {
      seq: 1,
      entry: 'plan',
      visit: 1,
      startedAt: '2026-11-16T09:00:00Z',
      end: {
        outcome: 'done',
        output: 'Add the column.',
        endedAt: '2026-11-16T09:01:00Z',
      },
    },
    writing,
  ],
};

const DONE: RunActionResult = { ok: true, value: undefined };

function mount(steerStep: RunsContext['steerStep'] = vi.fn(async () => DONE)) {
  const [run, setRun] = createSignal<Run>(runningRun);
  const context: RunsContext = {
    taskRuns: () => ({
      state: () => 'ready',
      data: () => [run()],
      retry: vi.fn(),
    }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => ({ waiting: [], starting: [] }),
      retry: vi.fn(),
    }),
    run: () => ({ state: () => 'ready', data: run, retry: vi.fn() }),
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
    steerStep,
  };
  render(() => (
    <RunsProvider value={context}>
      <RunView runId="run-1" onClose={vi.fn()} />
    </RunsProvider>
  ));
  const withTerminal = (terminal: RunStep['terminal'], end?: RunStep['end']) =>
    setRun({
      ...runningRun,
      steps: [runningRun.steps[0]!, { ...writing, terminal, end }],
    });
  return { steerStep, setRun, withTerminal };
}

async function openTerminal() {
  fireEvent.click(await screen.findByRole('button', { name: /^Watch Write/ }));
  return screen.findByRole('dialog', { name: 'Write: the agent at work' });
}

function lastSteer(steerStep: RunsContext['steerStep']): SteerStepInput {
  const call = vi.mocked(steerStep).mock.calls.at(-1);
  if (!call) throw new Error('Nothing was steered');
  return call[0];
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('Step Terminal', () => {
  it('opens a running Step’s agent session from the Run view', async () => {
    mount();

    // Only the running Step with a session offers it.
    expect(
      await screen.findAllByRole('button', { name: /^Watch/ })
    ).toHaveLength(1);
    await openTerminal();

    const session = document.querySelector('[data-agent-session]');
    expect(session?.getAttribute('data-agent-session')).toBe('session-2');
    const transcript = screen.getByText('The agent’s transcript');
    // Read-only: nothing quotes the transcript into a composer.
    expect(transcript.getAttribute('data-replies')).toBe('false');
  });

  it('sends the agent a note and clears it', async () => {
    const { steerStep } = mount();
    await openTerminal();

    const note = screen.getByRole('textbox', { name: 'Note to the agent' });
    const send = screen.getByRole('button', { name: 'Send note' });
    expect(send.hasAttribute('disabled')).toBe(true);
    fireEvent.input(note, { target: { value: '  Use the theme tokens ' } });
    fireEvent.click(send);

    await waitFor(() =>
      expect(lastSteer(steerStep)).toEqual({
        runId: 'run-1',
        step: expect.objectContaining({ seq: 2 }),
        action: 'note',
        note: 'Use the theme tokens',
      })
    );
    await waitFor(() => expect((note as HTMLTextAreaElement).value).toBe(''));
  });

  it('pauses the Step, then resumes it with the note', async () => {
    const { steerStep, withTerminal } = mount();
    await openTerminal();

    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() => expect(lastSteer(steerStep).action).toBe('pause'));

    withTerminal({
      sessionId: 'session-2',
      pausedAt: '2026-11-16T09:02:00Z',
      steers: [
        { action: 'pause', undelivered: false, at: '2026-11-16T09:02:00Z' },
      ],
    });
    // While paused, a note goes with the resume.
    expect(await screen.findByRole('button', { name: 'Resume' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Send note' })).toBeNull();
    expect(screen.getAllByText('Paused').length).toBeGreaterThan(0);
    fireEvent.input(
      screen.getByRole('textbox', { name: 'Note to the agent' }),
      {
        target: { value: 'Go on with the docs too' },
      }
    );
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }));

    await waitFor(() =>
      expect(lastSteer(steerStep)).toMatchObject({
        action: 'resume',
        note: 'Go on with the docs too',
      })
    );
    expect(screen.getByText('You paused the step')).toBeTruthy();
  });

  it('stops the Step only once the person confirms', async () => {
    const { steerStep, withTerminal } = mount();
    await openTerminal();

    fireEvent.click(screen.getByRole('button', { name: 'Stop step' }));
    expect(steerStep).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Keep it running' }));
    fireEvent.click(screen.getByRole('button', { name: 'Stop step' }));
    const confirm = screen.getAllByRole('button', { name: 'Stop step' }).at(-1);
    if (!confirm) throw new Error('No confirmation');
    fireEvent.click(confirm);

    await waitFor(() => expect(lastSteer(steerStep).action).toBe('stop'));
    withTerminal({
      sessionId: 'session-2',
      stopAskedAt: '2026-11-16T09:03:00Z',
      steers: [
        { action: 'stop', undelivered: false, at: '2026-11-16T09:03:00Z' },
      ],
    });
    expect(
      await screen.findByText('The step is stopping, and will end as failed.')
    ).toBeTruthy();
    expect(
      screen.queryByRole('textbox', { name: 'Note to the agent' })
    ).toBeNull();
  });

  it('says when a paused Step ends, and why it ended', async () => {
    const { withTerminal } = mount();
    await openTerminal();
    withTerminal({
      sessionId: 'session-2',
      pausedAt: '2026-11-16T09:02:00Z',
      endsBy: '2026-11-16T10:51:00Z',
      steers: [],
    });
    expect(await screen.findByText(/Its time runs out at /)).toBeTruthy();

    withTerminal(
      {
        pausedAt: '2026-11-16T09:02:00Z',
        pauseRanOutAt: '2026-11-16T10:51:00Z',
        steers: [],
      },
      {
        outcome: 'failed',
        output: 'The step stayed paused past its time.',
        endedAt: '2026-11-16T10:51:00Z',
      }
    );
    expect(
      await screen.findByText(
        'The step stayed paused past its time, and ended as failed.'
      )
    ).toBeTruthy();
  });

  it('keeps the controls, disabled, while the agent is between turns', async () => {
    const { steerStep, withTerminal } = mount();
    await openTerminal();
    withTerminal({ sessionId: 'session-2', betweenTurns: true, steers: [] });

    expect(await screen.findByText(/The agent is between turns/)).toBeTruthy();
    for (const name of ['Stop step', 'Pause', 'Send note']) {
      expect(
        screen.getByRole('button', { name }).hasAttribute('disabled')
      ).toBe(true);
    }
    fireEvent.click(screen.getByRole('button', { name: 'Stop step' }));
    expect(steerStep).not.toHaveBeenCalled();
  });

  it('says why the Step took no steering, and that it ended', async () => {
    const steerStep = vi.fn(
      async (): Promise<RunActionResult> => ({ ok: false, reason: 'conflict' })
    );
    const { withTerminal } = mount(steerStep);
    await openTerminal();

    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith('The step can’t take that now', {
        subtext:
          'It ended, is paused, or its agent is between turns. The run shows where it is.',
      })
    );

    withTerminal(
      { steers: [] },
      { outcome: 'done', output: 'Wrote it.', endedAt: '2026-11-16T09:10:00Z' }
    );
    expect(
      await screen.findByText(
        'This step has ended. Its agent’s session is closed.'
      )
    ).toBeTruthy();
    expect(document.querySelector('[data-agent-session]')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Pause' })).toBeNull();
  });
});
