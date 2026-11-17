/** @vitest-environment jsdom */
import { toast } from '@core/component/Toast/Toast';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@solidjs/testing-library';
import { type Accessor, createSignal } from 'solid-js';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import {
  type RunActionResult,
  type RunsContext,
  RunsProvider,
} from './context/runs-context';
import type { Run } from './core/run';
import { RunView } from './run-view';

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { alert: vi.fn(), failure: vi.fn(), success: vi.fn() },
}));

beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

// The shared collapse animation measures with ResizeObserver, which jsdom lacks.
vi.stubGlobal(
  'ResizeObserver',
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
);

const waitingRun: Run = {
  id: 'run-1',
  request: 'Add a due date column\n\nShow it in the task list.',
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
        output: 'Add a `due_date` column to the list.',
        endedAt: '2026-11-16T09:01:00Z',
      },
      gate: {
        question: 'Approve this plan, or send it back with a note?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-16T09:01:00Z',
      },
    },
  ],
};

const DONE: RunActionResult = { ok: true, value: undefined };

function mount(options: {
  answerGate?: RunsContext['answerGate'];
  cancelRun?: RunsContext['cancelRun'];
  state?: Accessor<'pending' | 'error' | 'ready'>;
  retry?: () => void;
}) {
  const [run, setRun] = createSignal<Run | undefined>(waitingRun);
  const context: RunsContext = {
    taskRuns: () => ({
      state: () => 'ready',
      data: () => [waitingRun],
      retry: vi.fn(),
    }),
    run: () => ({
      state: options.state ?? (() => 'ready'),
      data: () =>
        options.state && options.state() !== 'ready' ? undefined : run(),
      retry: options.retry ?? vi.fn(),
    }),
    flows: () => ({ state: () => 'ready', data: () => [], retry: vi.fn() }),
    repositories: () => ({
      state: () => 'ready',
      data: () => [],
      retry: vi.fn(),
    }),
    startRun: vi.fn(),
    answerGate: options.answerGate ?? vi.fn(async () => DONE),
    cancelRun: options.cancelRun ?? vi.fn(async () => DONE),
  };
  render(() => (
    <RunsProvider value={context}>
      <RunView runId="run-1" onClose={vi.fn()} />
    </RunsProvider>
  ));
  return { context, setRun };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('RunView', () => {
  it('shows a round of checks left open by an ended Step as unfinished', async () => {
    const { setRun } = mount({});
    setRun({
      ...waitingRun,
      end: { outcome: 'manual', endedAt: '2026-11-16T11:00:00Z' },
      steps: [
        {
          seq: 1,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:00:00Z',
          end: {
            outcome: 'red',
            output: 'The checks did not pass in time.',
            endedAt: '2026-11-16T11:00:00Z',
          },
          checks: [
            {
              round: 1,
              checks: ['web'],
              startedAt: '2026-11-16T09:30:00Z',
            },
          ],
        },
      ],
    });

    expect(await screen.findByText('Checks, round 1')).toBeTruthy();
    expect(screen.getByText('Didn’t finish')).toBeTruthy();
    expect(screen.queryByText('Checks running…')).toBeNull();
  });

  it('answers the open Gate once, with the note', async () => {
    let finish!: (result: RunActionResult) => void;
    const answerGate = vi.fn(
      () =>
        new Promise<RunActionResult>((resolve) => {
          finish = resolve;
        })
    );
    mount({ answerGate });

    expect(
      await screen.findByText('Approve this plan, or send it back with a note?')
    ).toBeTruthy();
    // The plan the Gate asks about is open to read.
    expect(screen.getByText('due_date')).toBeTruthy();

    fireEvent.input(
      screen.getByPlaceholderText('Add a note for the next steps (optional)'),
      { target: { value: 'Keep the column narrow' } }
    );
    fireEvent.click(screen.getByRole('button', { name: 'Revise' }));
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));

    expect(answerGate).toHaveBeenCalledTimes(1);
    expect(answerGate).toHaveBeenCalledWith({
      runId: 'run-1',
      step: waitingRun.steps[0],
      answer: 'revise',
      note: 'Keep the column narrow',
    });

    finish({ ok: true, value: undefined });
    const note = screen.getByPlaceholderText(
      'Add a note for the next steps (optional)'
    );
    await waitFor(() => expect((note as HTMLTextAreaElement).value).toBe(''));
    // Until the Run is read past it, the answered Gate stays locked.
    expect(
      screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
    ).toBe(true);
  });

  it('says so when someone else answered first', async () => {
    mount({
      answerGate: vi.fn(async () => ({
        ok: false as const,
        reason: 'conflict' as const,
      })),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }));

    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith(
        'This question was already answered',
        { subtext: 'The run has moved on and now shows where it is' }
      )
    );
    expect(toast.failure).not.toHaveBeenCalled();
  });

  it('cancels only after the person confirms', async () => {
    const cancelRun = vi.fn(async () => ({
      ok: false as const,
      reason: 'conflict' as const,
    }));
    mount({ cancelRun });

    fireEvent.click(await screen.findByRole('button', { name: 'Cancel run' }));
    fireEvent.click(
      await screen.findByRole('button', { name: 'Keep running' })
    );
    expect(screen.queryByText('Cancel this run?')).toBeNull();
    expect(cancelRun).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel run' }));
    expect(screen.getByText('Cancel this run?')).toBeTruthy();
    const confirm = screen
      .getAllByRole('button', { name: 'Cancel run' })
      .find((button) => button.getAttribute('data-variant') === 'danger');
    if (!confirm) throw new Error('No confirm button');
    fireEvent.click(confirm);

    expect(cancelRun).toHaveBeenCalledTimes(1);
    expect(cancelRun).toHaveBeenCalledWith('run-1');
    await waitFor(() =>
      expect(toast.alert).toHaveBeenCalledWith('This run has already ended')
    );
  });

  it('unlocks the answers once the Run is read at its next Gate', async () => {
    const { setRun } = mount({});

    fireEvent.click(await screen.findByRole('button', { name: 'Revise' }));
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
      ).toBe(true)
    );

    // The service took the answer and planned again: a new Gate opens.
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            answer: { answer: 'revise', answeredAt: '2026-11-16T09:02:00Z' },
          },
        },
        { ...first, seq: 2, visit: 2, startedAt: '2026-11-16T09:02:00Z' },
      ],
    });

    expect(
      screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
    ).toBe(false);
  });

  it('names the repository a Run works on, and the branch it starts from', () => {
    const { setRun } = mount({});
    expect(screen.queryByText('Repository')).toBeNull();

    setRun({
      ...waitingRun,
      repository: {
        url: 'https://github.com/macro-inc/app',
        baseBranch: 'develop',
      },
    });

    expect(screen.getByText('Repository')).toBeTruthy();
    expect(screen.getByText('macro-inc/app, from develop')).toBeTruthy();
  });

  it('retries a Run that couldn’t be loaded', () => {
    const [state, setState] = createSignal<'pending' | 'error' | 'ready'>(
      'error'
    );
    const retry = vi.fn(() => setState('ready'));
    mount({ state, retry });

    expect(screen.getByText('This run couldn’t be loaded')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(retry).toHaveBeenCalledOnce();
    expect(
      screen.getByText('Approve this plan, or send it back with a note?')
    ).toBeTruthy();
  });

  it('offers the Flow’s main answer last, as the primary action', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: { ...first.gate, answers: ['approve', 'revise', 'reject'] },
        },
      ],
    });

    const group = await screen.findByRole('group', {
      name: 'Your answer is needed',
    });
    const answers = within(group)
      .getAllByRole('button')
      .map((button) => button.textContent);
    expect(answers).toEqual(['Reject', 'Revise', 'Approve']);
  });

  it('shows the Role and Agent of each Step, and who was passed over', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first) throw new Error('No first Step');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          role: 'planner',
          agent: 'backup',
          passedOver: [{ agent: 'primary', reason: 'ai_allowance_exhausted' }],
        },
        {
          ...first,
          seq: 2,
          visit: 2,
          role: 'planner',
          agent: 'backup',
          passedOver: [
            { agent: 'primary', reason: 'ai_allowance_exhausted' },
            { agent: 'backup', reason: 'ai_overage_limit_reached' },
          ],
        },
      ],
    });

    expect(await screen.findByText('Planner · backup')).toBeTruthy();
    expect(
      screen.getAllByText('primary couldn’t start: AI allowance exhausted')
    ).toHaveLength(2);
    // When the last Agent couldn't start either, none took the Step.
    expect(screen.getByText('Planner · no agent could start')).toBeTruthy();
    expect(
      screen.getByText('backup couldn’t start: AI overage limit reached')
    ).toBeTruthy();
  });

  it('says so when an answer fails for another reason', async () => {
    mount({
      answerGate: vi.fn(async () => ({
        ok: false as const,
        reason: 'failed' as const,
      })),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }));

    await waitFor(() =>
      expect(toast.failure).toHaveBeenCalledWith('Couldn’t send your answer')
    );
    expect(toast.alert).not.toHaveBeenCalled();
    // A failed answer leaves the Gate open to answer again.
    expect(
      screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
    ).toBe(false);
  });

  it('keeps Cancel run locked after a cancel until the Run reads as ended', async () => {
    const { setRun } = mount({});

    fireEvent.click(await screen.findByRole('button', { name: 'Cancel run' }));
    const confirm = screen
      .getAllByRole('button', { name: 'Cancel run' })
      .find((button) => button.getAttribute('data-variant') === 'danger');
    if (!confirm) throw new Error('No confirm button');
    fireEvent.click(confirm);

    await waitFor(() => expect(confirm.getAttribute('aria-busy')).toBe('true'));
    expect(screen.getByRole('button', { name: 'Keep running' })).toHaveProperty(
      'disabled',
      true
    );

    setRun({
      ...waitingRun,
      end: { outcome: 'cancelled', endedAt: '2026-11-16T09:05:00Z' },
    });
    expect(screen.queryByRole('button', { name: 'Cancel run' })).toBeNull();
  });
  it('leaves focus where it is when a poll moves the Run on', async () => {
    const { setRun } = mount({});
    await screen.findByRole('button', { name: 'Approve' });
    const status = document.querySelector<HTMLElement>('[data-run-status]');
    if (!status) throw new Error('No status line');
    // Where focus lands when a control goes away, or a click on text.
    screen.getByRole('dialog').focus();

    // Someone else answered the Gate; a poll reads the Run past it.
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            answer: { answer: 'approve', answeredAt: '2026-11-16T09:02:00Z' },
          },
        },
        {
          seq: 2,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:02:00Z',
        },
      ],
    });
    await new Promise<void>((resolve) => queueMicrotask(resolve));
    await new Promise<void>((resolve) => queueMicrotask(resolve));

    expect(document.activeElement).not.toBe(status);
  });

  it('leaves focus alone on a later poll after a 409 that kept its Gate', async () => {
    const { setRun } = mount({
      answerGate: vi.fn(async () => ({
        ok: false as const,
        reason: 'conflict' as const,
      })),
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }));
    await waitFor(() => expect(toast.alert).toHaveBeenCalled());
    const status = document.querySelector<HTMLElement>('[data-run-status]');
    screen.getByRole('dialog').focus();

    // Later, someone else answers and a poll reads the Run past the Gate.
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            answer: { answer: 'revise', answeredAt: '2026-11-16T09:09:00Z' },
          },
        },
        { ...first, seq: 2, visit: 2, gate: undefined },
      ],
    });
    await new Promise<void>((resolve) => queueMicrotask(resolve));
    await new Promise<void>((resolve) => queueMicrotask(resolve));

    expect(document.activeElement).not.toBe(status);
  });

  it('unlocks a Gate that a reset brings back at the same visit', async () => {
    const { setRun } = mount({});
    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }));
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
      ).toBe(true)
    );

    // A reset restarts the entry's visits: its next Step is visit 1 again,
    // a new Step all the same.
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            answer: { answer: 'approve', answeredAt: '2026-11-16T09:02:00Z' },
          },
        },
        {
          seq: 2,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:02:00Z',
          end: {
            outcome: 'done',
            output: 'Wrote it.',
            endedAt: '2026-11-16T09:03:00Z',
          },
        },
        { ...first, seq: 3, startedAt: '2026-11-16T09:03:00Z' },
      ],
    });

    expect(
      screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
    ).toBe(false);
  });
});
