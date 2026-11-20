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
import type { Run, RunStep } from './core/run';
import { RunView } from './run-view';

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
  ownerId: 'macro|julia@example.com',
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
        kind: 'flow',
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
  replyToGate?: RunsContext['replyToGate'];
  cancelRun?: RunsContext['cancelRun'];
  markRunFlawed?: RunsContext['markRunFlawed'];
  state?: Accessor<'pending' | 'error' | 'ready'>;
  retry?: () => void;
  viewerId?: string;
}) {
  const [run, setRun] = createSignal<Run | undefined>(waitingRun);
  const context: RunsContext = {
    viewerId: () => options.viewerId ?? 'macro|julia@example.com',
    taskRuns: () => ({
      state: () => 'ready',
      data: () => [waitingRun],
      retry: vi.fn(),
    }),
    waitingRuns: () => ({
      state: () => 'ready',
      data: () => ({ waiting: [waitingRun], starting: [] }),
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
    replyToGate: options.replyToGate ?? vi.fn(async () => DONE),
    cancelRun: options.cancelRun ?? vi.fn(async () => DONE),
    markRunFlawed: options.markRunFlawed ?? vi.fn(async () => DONE),
    steerStep: vi.fn(async () => DONE),
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
  touch.device = false;
});

describe('RunView', () => {
  it('shows a Rule Answer with the standing rule that gave it, and asks no one', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            answer: {
              answer: 'approve',
              rule: 'approve_plan',
              answeredAt: '2026-11-16T09:01:00Z',
            },
          },
        },
        {
          seq: 2,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:01:00Z',
        },
      ],
    });

    expect(
      await screen.findByText(
        'Answered by the standing rule “Approve plans”: Approve'
      )
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull();
  });

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
              verified: false,
              results: [],
            },
          ],
        },
      ],
    });

    expect(await screen.findByText('Checks, round 1')).toBeTruthy();
    expect(screen.getByText('Didn’t finish')).toBeTruthy();
    expect(screen.queryByText('Checks running…')).toBeNull();
  });

  it('shows each review round with its blockers and follow-ups', async () => {
    const { setRun } = mount({});
    const reviewer = (seq: number, role: string, outcome: string) => ({
      seq,
      entry: 'review',
      visit: 1,
      startedAt: '2026-11-16T10:00:00Z',
      role,
      end: {
        outcome,
        output: `${role} checked the change.`,
        endedAt: '2026-11-16T10:05:00Z',
      },
    });
    setRun({
      ...waitingRun,
      steps: [
        reviewer(3, 'correctness-reviewer', 'changes'),
        reviewer(4, 'tests-reviewer', 'changes'),
      ],
      reviewRounds: [
        {
          round: 2,
          visit: 2,
          seq: 3,
          reviewers: 2,
          headSha: 'bbbbbbbbbb',
          diffBase: 'aaaaaaaaaa',
          fixDiff: true,
          startedAt: '2026-11-16T10:00:00Z',
          end: { outcome: 'changes', endedAt: '2026-11-16T10:06:00Z' },
          findings: [
            {
              seq: 3,
              place: 1,
              role: 'correctness-reviewer',
              path: 'src/save.rs',
              lines: { start: 12, end: 14 },
              problem: 'The save drops its error',
              why: 'A failed save looks saved',
              blocksAnywhere: false,
              kind: 'blocker',
            },
            {
              seq: 4,
              place: 1,
              role: 'tests-reviewer',
              path: 'src/load.rs',
              problem: 'Loading has no test',
              why: 'A broken load would pass',
              blocksAnywhere: false,
              kind: 'follow_up',
              followUpTaskId: 'task-2',
            },
          ],
        },
      ],
    });

    expect(await screen.findByText('Review round 2')).toBeTruthy();
    expect(
      screen.getByText(
        /Reviews the fix since aaaaaaa · 1 blocker · 1 follow-up/
      )
    ).toBeTruthy();
    expect(screen.getByText('src/save.rs, lines 12–14')).toBeTruthy();
    expect(screen.getByText('The save drops its error')).toBeTruthy();
    expect(screen.getByText('Blocker')).toBeTruthy();
    expect(screen.getByText('A failed save looks saved')).toBeTruthy();
    const task = screen.getByText('Follow-up task').closest('a');
    expect(task?.getAttribute('href')).toMatch(/\/app\/task\/task-2$/);
    expect(screen.getByText('Loading has no test')).toBeTruthy();
  });

  it('shows what the fix said of each blocker, the ruling, and the Run’s counts', async () => {
    const { setRun } = mount({});
    const blocker = (
      place: number,
      path: string,
      extra: Record<string, unknown>
    ) => ({
      seq: 3,
      place,
      role: 'correctness-reviewer',
      path,
      problem: `${path} drops its error`,
      why: 'A failed save looks saved',
      blocksAnywhere: false,
      kind: 'blocker' as const,
      ...extra,
    });
    setRun({
      ...waitingRun,
      steps: [
        {
          seq: 3,
          entry: 'review',
          visit: 1,
          startedAt: '2026-11-16T10:00:00Z',
          role: 'correctness-reviewer',
        },
      ],
      reviewRounds: [
        {
          round: 1,
          visit: 1,
          seq: 3,
          reviewers: 1,
          headSha: 'aaaaaaaaaa',
          diffBase: 'main',
          fixDiff: false,
          startedAt: '2026-11-16T10:00:00Z',
          end: { outcome: 'changes', endedAt: '2026-11-16T10:06:00Z' },
          findings: [
            blocker(1, 'src/a.rs', { fixStatus: 'fixed' }),
            blocker(2, 'src/b.rs', {
              fixStatus: 'disputed',
              dispute: 'The sandbox cannot reach production',
              ruling: 'dropped',
            }),
            blocker(3, 'src/c.rs', {
              fixStatus: 'disputed',
              dispute: 'It is generated',
              ruling: 'upheld',
            }),
            blocker(4, 'src/d.rs', { kind: 'dismissed' }),
          ],
        },
      ],
      blockerResolutions: { fixed: 1, disputedUpheld: 1, disputedDropped: 1, dismissed: 1 },
    });

    expect(await screen.findByText('Review round 1')).toBeTruthy();
    expect(screen.getByText('The fixer says it fixed this')).toBeTruthy();
    expect(
      screen.getByText(
        'The fixer disputes this: The sandbox cannot reach production'
      )
    ).toBeTruthy();
    expect(
      screen.getByText('The next round dropped this blocker: it blocked nothing')
    ).toBeTruthy();
    expect(
      screen.getByText('The next round upheld this blocker: it blocked again')
    ).toBeTruthy();
    expect(screen.getByText('Dismissed')).toBeTruthy();
    expect(
      screen.getByText(
        '1 fixed, as the fixer says · 1 disputed, then upheld · 1 disputed, then dropped · 1 dismissed as repeats'
      )
    ).toBeTruthy();
  });

  it('labels only a review round after a reset as a fresh review', async () => {
    const { setRun } = mount({});
    const reviewer = (seq: number, entry: string, visit: number) => ({
      seq,
      entry,
      visit,
      startedAt: '2026-11-16T11:00:00Z',
      role: 'correctness-reviewer',
    });
    const round = (
      number: number,
      visit: number,
      seq: number,
      fixDiff: boolean
    ) => ({
      round: number,
      visit,
      seq,
      reviewers: 1,
      headSha: 'cccccccccc',
      diffBase: fixDiff ? 'bbbbbbbbbb' : 'main',
      fixDiff,
      startedAt: '2026-11-16T11:00:00Z',
      findings: [],
    });
    setRun({
      ...waitingRun,
      // Two rounds of `review`, a reset sending it back to visit 1, then
      // the first round of a second panel entry.
      steps: [
        reviewer(3, 'review', 1),
        reviewer(6, 'review', 2),
        reviewer(9, 'review', 1),
        reviewer(12, 'final-review', 1),
      ],
      reviewRounds: [
        round(1, 1, 3, false),
        round(2, 2, 6, true),
        round(3, 1, 9, false),
        round(4, 1, 12, false),
      ],
    });

    expect(
      await screen.findByText('Review round 3, a fresh review')
    ).toBeTruthy();
    expect(screen.getByText('Review round 1')).toBeTruthy();
    expect(screen.getByText('Review round 2')).toBeTruthy();
    expect(screen.getByText('Review round 4')).toBeTruthy();
    expect(screen.queryAllByText(/a fresh review/)).toHaveLength(1);
  });

  it('says who ran each round, and the exit code of each check that failed', async () => {
    const { setRun } = mount({});
    setRun({
      ...waitingRun,
      steps: [
        {
          seq: 1,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:00:00Z',
          checks: [
            {
              round: 1,
              checks: ['web', 'lint'],
              startedAt: '2026-11-16T09:30:00Z',
              verified: true,
              checkedCommit: '0123456789abcdef0123456789abcdef01234567',
              results: [
                { name: 'web', exitCode: 0 },
                { name: 'lint', exitCode: 2 },
              ],
              end: {
                outcome: 'failed',
                report: 'lint failed',
                endedAt: '2026-11-16T09:40:00Z',
              },
            },
            {
              round: 2,
              checks: ['web', 'lint'],
              startedAt: '2026-11-16T09:50:00Z',
              verified: false,
              results: [],
              end: {
                outcome: 'passed',
                report: 'web and lint passed',
                endedAt: '2026-11-16T10:00:00Z',
              },
            },
            {
              round: 3,
              checks: ['web'],
              startedAt: '2026-11-16T10:10:00Z',
              verified: false,
              results: [],
            },
          ],
        },
      ],
    });

    expect(await screen.findByText('Checks, round 1')).toBeTruthy();
    expect(screen.getByText('Run on pushed commit 0123456')).toBeTruthy();
    expect(screen.getByText('web, lint exited 2')).toBeTruthy();
    // Only the ended round the agent reported says so; the open one runs.
    expect(screen.getAllByText('Reported by the agent')).toHaveLength(1);
    expect(screen.getByText('Checks running…')).toBeTruthy();
  });

  it('names the setup command that failed before the round ran its checks', async () => {
    const { setRun } = mount({});
    setRun({
      ...waitingRun,
      steps: [
        {
          seq: 1,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:00:00Z',
          checks: [
            {
              round: 1,
              checks: ['web'],
              startedAt: '2026-11-16T09:30:00Z',
              verified: true,
              checkedCommit: '0123456789abcdef0123456789abcdef01234567',
              results: [{ name: 'setup:install', exitCode: 1 }],
              end: {
                outcome: 'failed',
                report: 'the setup failed',
                endedAt: '2026-11-16T09:40:00Z',
              },
            },
          ],
        },
      ],
    });

    expect(await screen.findByText('setup install exited 1, web')).toBeTruthy();
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

  it('sends the note as a reply, and keeps the Gate open for the answer it reads', async () => {
    const replyToGate = vi.fn(async () => DONE);
    const answerGate = vi.fn(async () => DONE);
    mount({ replyToGate, answerGate });

    const send = await screen.findByRole('button', { name: 'Send as reply' });
    // With no words there's nothing to send.
    expect(send.hasAttribute('disabled')).toBe(true);
    fireEvent.input(
      screen.getByPlaceholderText('Add a note for the next steps (optional)'),
      { target: { value: '  Looks right, go ahead  ' } }
    );
    fireEvent.click(send);

    await waitFor(() =>
      expect(replyToGate).toHaveBeenCalledWith({
        runId: 'run-1',
        step: waitingRun.steps[0],
        reply: 'Looks right, go ahead',
      })
    );
    expect(answerGate).not.toHaveBeenCalled();
    // The factory may ask back: the Gate still takes an answer.
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Approve' }).hasAttribute('disabled')
      ).toBe(false)
    );
  });

  it('shows words the factory asked back on, and the stand-in’s answer with its window', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            askedBack: {
              reply: 'Hmm',
              at: '2026-11-16T09:02:00Z',
              refusedTampered: false,
            },
          },
        },
      ],
    });
    expect(
      await screen.findByText(
        'Couldn’t tell which answer “Hmm” gives. Pick an answer, or say it another way.'
      )
    ).toBeTruthy();

    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            standIn: {
              answer: 'approve',
              reason: 'You approved every plan like this one.',
              confidence: 93,
              until: '2026-11-16T09:11:00Z',
            },
          },
        },
      ],
    });
    expect(
      await screen.findByText(
        'The stand-in answered Approve for you (93% sure)'
      )
    ).toBeTruthy();
    expect(
      screen.getByText('You approved every plan like this one.')
    ).toBeTruthy();
    // The owner can still override it.
    expect(
      screen.getByRole('button', { name: 'Revise' }).hasAttribute('disabled')
    ).toBe(false);
  });

  it('says why the stand-in left a Gate, and how the words asked back on were read', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            standInDeclined: {
              pick: 'approve',
              confidence: 62,
              reason: 'The plan leaves the migration out.',
            },
            askedBack: {
              reply: 'Hmm',
              at: '2026-11-16T09:02:00Z',
              refusedTampered: false,
              reading: {
                confidence: 40,
                reason: 'The reply asks a question back.',
              },
            },
          },
        },
      ],
    });
    expect(
      await screen.findByText('The stand-in left this one to you.')
    ).toBeTruthy();
    expect(
      screen.getByText(
        'It leaned toward Approve (62% sure). The plan leaves the migration out.'
      )
    ).toBeTruthy();
    expect(
      screen.getByText(
        'It couldn’t tell which answer fits. The reply asks a question back.'
      )
    ).toBeTruthy();

    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            standInDeclined: { reason: '' },
          },
        },
      ],
    });
    expect(await screen.findByText('It didn’t answer.')).toBeTruthy();
  });

  it('says why words approving a Tampered Run were asked back', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            askedBack: {
              reply: 'Approve',
              at: '2026-11-16T09:05:00Z',
              reading: { pick: 'approve', confidence: 98, reason: 'Clear.' },
              refusedTampered: true,
            },
          },
        },
      ],
    });
    expect(
      await screen.findByText(
        'This run changed its Control Files, so approving it needs the Approve button, which lists them.'
      )
    ).toBeTruthy();
  });

  it('shows words asked back on in the stand-in’s window, not the window', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            standIn: {
              answer: 'approve',
              reason: 'You approved every plan like this one.',
              confidence: 93,
              until: '2026-11-16T09:11:00Z',
            },
            askedBack: {
              reply: 'Hmm',
              at: '2026-11-16T09:05:00Z',
              refusedTampered: false,
              reading: {
                pick: 'revise',
                confidence: 70,
                reason: 'The reply doubts the plan.',
              },
            },
          },
        },
      ],
    });
    expect(
      await screen.findByText(
        'It leaned toward Revise (70% sure). The reply doubts the plan.'
      )
    ).toBeTruthy();
    expect(
      screen.queryByText('The stand-in answered Approve for you (93% sure)')
    ).toBeNull();
  });

  it('says when the stand-in answered, and when an answer was read from a reply', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    const standIn = {
      answer: 'approve',
      reason: 'You approved every plan like this one.',
      confidence: 93,
      until: '2026-11-16T09:11:00Z',
    };
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            standIn,
            answer: {
              answer: 'approve',
              byStandIn: true,
              answeredAt: '2026-11-16T09:11:00Z',
            },
          },
        },
        {
          ...first,
          seq: 2,
          visit: 2,
          gate: {
            ...first.gate,
            answer: {
              answer: 'revise',
              note: 'It needs tests first',
              replyConfidence: 91,
              answeredAt: '2026-11-16T09:12:00Z',
            },
          },
        },
        {
          seq: 3,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T09:12:00Z',
        },
      ],
    });

    expect(
      await screen.findByText('Answered for you by the stand-in: Approve')
    ).toBeTruthy();
    expect(
      screen.getByText('You approved every plan like this one.')
    ).toBeTruthy();
    expect(
      screen.getByText('Answered from your reply (91% sure): Revise')
    ).toBeTruthy();
  });

  it('says when the Orchestrator answered', async () => {
    const { setRun } = mount({});
    const [first] = waitingRun.steps;
    if (!first?.gate) throw new Error('No first Gate');
    setRun({
      ...waitingRun,
      steps: [
        {
          ...first,
          gate: {
            ...first.gate,
            answer: {
              answer: 'revise',
              byOrchestrator: true,
              answeredAt: '2026-11-16T09:11:00Z',
            },
          },
        },
      ],
    });

    expect(
      await screen.findByText('Answered by your Orchestrator: Revise')
    ).toBeTruthy();
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

  it('shows a Run’s token budget, what its agents used, and why it stopped', () => {
    const { setRun } = mount({});
    expect(screen.queryByText('Token budget')).toBeNull();

    setRun({
      ...waitingRun,
      tokenBudget: { budget: 2_000_000, used: 1_250_000 },
    });
    expect(screen.getByText('Token budget')).toBeTruthy();
    expect(screen.getByText('1.3M of 2M tokens used')).toBeTruthy();

    setRun({ ...waitingRun, tokenBudget: { budget: 2_000_000 } });
    expect(screen.getByText('2M tokens')).toBeTruthy();

    setRun({
      ...waitingRun,
      tokenBudget: { budget: 2_000_000, used: 0, measured: false },
    });
    expect(
      screen.getByText(
        '2M tokens, not measured: its agents record no token use, so the budget can’t stop it'
      )
    ).toBeTruthy();

    setRun({
      ...waitingRun,
      tokenBudget: {
        budget: 2_000_000,
        used: 2_100_000,
        stop: { reason: 'spent', used: 2_050_000 },
      },
    });
    expect(
      screen.getByText(
        'Stopped at its budget: its agents used 2.1M of 2M tokens'
      )
    ).toBeTruthy();

    setRun({
      ...waitingRun,
      tokenBudget: { budget: 2_000_000, stop: { reason: 'unread' } },
    });
    expect(
      screen.getByText(
        'Stopped: its token use couldn’t be read, so its budget of 2M tokens couldn’t be checked'
      )
    ).toBeTruthy();
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
    // Sending the note as a reply comes first, apart from the answers.
    expect(answers).toEqual(['Send as reply', 'Reject', 'Revise', 'Approve']);
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

  it('approves a Tampered Run only with the Control Files it saw approved', async () => {
    const answerGate = vi.fn(async () => DONE);
    const { setRun } = mount({ answerGate });
    setRun({
      ...waitingRun,
      controlFiles: {
        tampered: ['.github/workflows/ci.yml'],
        named: ['Makefile'],
        unlisted: false,
      },
    });

    expect(await screen.findByText('.github/workflows/ci.yml')).toBeTruthy();
    const named = screen.getByRole('list', {
      name: 'Control files named by the request',
    });
    expect(within(named).getByText('Makefile')).toBeTruthy();
    const approve = screen.getByRole('button', { name: 'Approve' });
    expect(approve.hasAttribute('disabled')).toBe(true);

    fireEvent.click(
      screen.getByRole('checkbox', { name: 'I approve these changes' })
    );
    await waitFor(() => expect(approve.hasAttribute('disabled')).toBe(false));
    fireEvent.click(approve);

    expect(answerGate).toHaveBeenCalledWith({
      runId: 'run-1',
      step: waitingRun.steps[0],
      answer: 'approve',
      note: undefined,
      approvedControlFiles: ['.github/workflows/ci.yml'],
    });
  });

  it('shows a held, Tampered merge with its review and verified checks', async () => {
    const answerGate = vi.fn(async () => DONE);
    const { setRun } = mount({ answerGate });
    const mergeGate: RunStep = {
      seq: 6,
      entry: 'merge-gate',
      visit: 1,
      startedAt: '2026-11-16T11:00:00Z',
      end: {
        outcome: 'done',
        output: 'Every check passed.',
        endedAt: '2026-11-16T11:01:00Z',
      },
      gate: {
        kind: 'flow',
        question: 'Merge this pull request?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-16T11:01:00Z',
      },
    };
    setRun({
      ...waitingRun,
      repository: {
        url: 'https://github.com/macro/app',
        baseBranch: 'main',
      },
      controlFiles: {
        tampered: ['.github/workflows/ci.yml'],
        named: [],
        unlisted: false,
      },
      mainHold: {
        since: '2026-11-16T10:30:00Z',
        headSha: '2222222222222222222222222222222222222222',
        failed: '- test (failure)',
        taskId: 'task-9',
      },
      steps: [
        {
          seq: 2,
          entry: 'write',
          visit: 1,
          startedAt: '2026-11-16T10:00:00Z',
          end: {
            outcome: 'done',
            output: 'Wrote it.',
            endedAt: '2026-11-16T10:20:00Z',
          },
          checks: [
            {
              round: 1,
              checks: ['web'],
              startedAt: '2026-11-16T10:10:00Z',
              verified: true,
              checkedCommit: '0123456789abcdef0123456789abcdef01234567',
              results: [{ name: 'web', exitCode: 0 }],
              end: {
                outcome: 'passed',
                report: 'web passed',
                endedAt: '2026-11-16T10:15:00Z',
              },
            },
          ],
        },
        {
          seq: 3,
          entry: 'review',
          visit: 1,
          startedAt: '2026-11-16T10:20:00Z',
          role: 'correctness-reviewer',
          end: {
            outcome: 'pass',
            output: 'Nothing blocks.',
            endedAt: '2026-11-16T10:25:00Z',
          },
        },
        mergeGate,
      ],
      reviewRounds: [
        {
          round: 1,
          visit: 1,
          seq: 3,
          reviewers: 1,
          headSha: '0123456789',
          diffBase: 'main',
          fixDiff: false,
          startedAt: '2026-11-16T10:20:00Z',
          end: { outcome: 'pass', endedAt: '2026-11-16T10:25:00Z' },
          findings: [],
        },
      ],
    });

    expect(await screen.findByText('Review round 1')).toBeTruthy();
    expect(screen.getByText('Run on pushed commit 0123456')).toBeTruthy();
    expect(screen.getByText('Main hold')).toBeTruthy();
    expect(screen.getByText('- test (failure)')).toBeTruthy();
    expect(
      screen
        .getByText('See the merges since they last passed')
        .closest('a')
        ?.getAttribute('href')
    ).toMatch(/\/app\/task\/task-9$/);
    expect(screen.getByText('.github/workflows/ci.yml')).toBeTruthy();

    const approve = screen.getByRole('button', { name: 'Approve' });
    expect(approve.hasAttribute('disabled')).toBe(true);
    fireEvent.click(
      screen.getByRole('checkbox', { name: 'I approve these changes' })
    );
    await waitFor(() => expect(approve.hasAttribute('disabled')).toBe(false));
    fireEvent.click(approve);

    expect(answerGate).toHaveBeenCalledWith({
      runId: 'run-1',
      step: mergeGate,
      answer: 'approve',
      note: undefined,
      approvedControlFiles: ['.github/workflows/ci.yml'],
    });
  });

  it('approves a Run that changed only named Control Files without asking', async () => {
    const answerGate = vi.fn(async () => DONE);
    const { setRun } = mount({ answerGate });
    setRun({
      ...waitingRun,
      controlFiles: { tampered: [], named: ['Makefile'], unlisted: false },
    });

    expect(await screen.findByText('Makefile')).toBeTruthy();
    expect(screen.queryByRole('checkbox')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));

    expect(answerGate).toHaveBeenCalledWith({
      runId: 'run-1',
      step: waitingRun.steps[0],
      answer: 'approve',
      note: undefined,
    });
  });

  it('sends a Tampered Run back without approving its Control Files', async () => {
    const answerGate = vi.fn(async () => DONE);
    const { setRun } = mount({ answerGate });
    setRun({
      ...waitingRun,
      controlFiles: { tampered: [], named: [], unlisted: true },
    });

    expect(
      await screen.findByText(/GitHub couldn’t list this branch’s changes/)
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Revise' }));

    expect(answerGate).toHaveBeenCalledWith({
      runId: 'run-1',
      step: waitingRun.steps[0],
      answer: 'revise',
      note: undefined,
    });
  });
  it('answers an agent’s question in the person’s own words', async () => {
    const answerGate = vi.fn(async () => DONE);
    const { setRun } = mount({ answerGate });
    const asking = {
      seq: 1,
      entry: 'plan',
      visit: 1,
      role: 'planner',
      startedAt: '2026-11-16T09:00:00Z',
      end: {
        outcome: 'question',
        output: 'Should the column show overdue dates in red?',
        endedAt: '2026-11-16T09:01:00Z',
      },
      gate: {
        kind: 'question' as const,
        question: 'Should the column show overdue dates in red?',
        answers: ['answer'],
        openedAt: '2026-11-16T09:01:00Z',
      },
    };
    setRun({ ...waitingRun, steps: [asking] });

    expect(await screen.findByText('The Planner has a question')).toBeTruthy();
    const send = screen.getByRole('button', { name: 'Answer' });
    // Nothing goes out until the person wrote an answer.
    expect(send.hasAttribute('disabled')).toBe(true);
    fireEvent.input(screen.getByPlaceholderText('Write your answer'), {
      target: { value: 'Yes, in red' },
    });
    await waitFor(() => expect(send.hasAttribute('disabled')).toBe(false));
    fireEvent.click(send);

    expect(answerGate).toHaveBeenCalledWith({
      runId: 'run-1',
      step: asking,
      answer: 'answer',
      note: 'Yes, in red',
    });
  });

  it('shows the answer to an agent’s question on its Step', async () => {
    const { setRun } = mount({});
    setRun({
      ...waitingRun,
      steps: [
        {
          seq: 1,
          entry: 'plan',
          visit: 1,
          role: 'planner',
          startedAt: '2026-11-16T09:00:00Z',
          end: {
            outcome: 'question',
            output: 'Should the column show overdue dates in red?',
            endedAt: '2026-11-16T09:01:00Z',
          },
          gate: {
            kind: 'question',
            question: 'Should the column show overdue dates in red?',
            answers: ['answer'],
            openedAt: '2026-11-16T09:01:00Z',
            answer: {
              answer: 'answer',
              note: 'Yes, in red',
              answeredAt: '2026-11-16T09:05:00Z',
            },
          },
        },
        {
          seq: 2,
          entry: 'plan',
          visit: 1,
          role: 'planner',
          startedAt: '2026-11-16T09:05:00Z',
        },
      ],
    });

    expect(await screen.findByText('Answered:')).toBeTruthy();
    expect(screen.getByText('Yes, in red')).toBeTruthy();
    expect(screen.queryByText('The Planner has a question')).toBeNull();
  });
  it('marks a run that ended done flawed, with what is wrong', async () => {
    const markRunFlawed = vi.fn(async () => DONE);
    const { setRun } = mount({ markRunFlawed });
    setRun({
      ...waitingRun,
      end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Mark flawed' }));
    const confirm = screen
      .getAllByRole('button', { name: 'Mark flawed' })
      .find((button) => button.getAttribute('data-variant') === 'danger');
    if (!confirm) throw new Error('No confirm button');
    // Nothing goes until the person says what is wrong.
    fireEvent.click(confirm);
    expect(markRunFlawed).not.toHaveBeenCalled();

    fireEvent.input(
      screen.getByRole('textbox', { name: 'What’s wrong with the result' }),
      { target: { value: '  The column is never filled  ' } }
    );
    fireEvent.click(confirm);

    await waitFor(() =>
      expect(markRunFlawed).toHaveBeenCalledWith({
        runId: 'run-1',
        reason: 'The column is never filled',
      })
    );
  });

  it('refuses a reason longer than the factory keeps', async () => {
    const markRunFlawed = vi.fn(async () => DONE);
    const { setRun } = mount({ markRunFlawed });
    setRun({
      ...waitingRun,
      end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
    });

    fireEvent.click(await screen.findByRole('button', { name: 'Mark flawed' }));
    fireEvent.input(
      screen.getByRole('textbox', { name: 'What’s wrong with the result' }),
      { target: { value: 'é'.repeat(2049) } }
    );
    const confirm = screen
      .getAllByRole('button', { name: 'Mark flawed' })
      .find((button) => button.getAttribute('data-variant') === 'danger');
    if (!confirm) throw new Error('No confirm button');
    fireEvent.click(confirm);

    expect(
      await screen.findByText(/The reason must be at most 4,?096 bytes/)
    ).toBeTruthy();
    expect(markRunFlawed).not.toHaveBeenCalled();
  });

  it.each([
    ['dialog', false],
    ['phone sheet', true],
  ])(
    'offers the flaw mark in the %s only to the run’s owner',
    async (_, device) => {
      touch.device = device;
      const succeeded: Run = {
        ...waitingRun,
        end: { outcome: 'success', endedAt: '2026-11-16T09:10:00Z' },
      };
      const teammate = mount({ viewerId: 'macro|teo@example.com' });
      teammate.setRun(succeeded);

      expect(
        await screen.findAllByText('Add a due date column')
      ).not.toHaveLength(0);
      expect(screen.queryByRole('button', { name: 'Mark flawed' })).toBeNull();

      cleanup();
      const owner = mount({});
      owner.setRun(succeeded);

      expect(
        await screen.findByRole('button', { name: 'Mark flawed' })
      ).toBeTruthy();
    }
  );

  it('offers no flaw mark for a run that did not end done', async () => {
    const { setRun } = mount({});
    setRun({
      ...waitingRun,
      end: { outcome: 'infrastructure', endedAt: '2026-11-16T09:10:00Z' },
    });

    expect(await screen.findByText('Infrastructure failure')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Mark flawed' })).toBeNull();
  });

  it('shows why a flawed run is flawed and which scorer said so', async () => {
    const { setRun } = mount({});
    setRun({
      ...waitingRun,
      end: {
        outcome: 'flawed',
        endedAt: '2026-11-16T09:10:00Z',
        flaw: {
          by: { kind: 'scorer', name: 'tests-pass' },
          reason: 'Two tests fail on main',
          markedAt: '2026-11-16T10:00:00Z',
        },
      },
    });

    expect(await screen.findByText('Two tests fail on main')).toBeTruthy();
    expect(
      screen.getByText(/Marked flawed by the tests-pass scorer/)
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Mark flawed' })).toBeNull();
  });
});
