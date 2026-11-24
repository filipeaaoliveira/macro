import { describe, expect, it } from 'vitest';
import {
  type BreakRecord,
  type BreakStatus,
  breakRecordsOf,
  gateAsker,
  isBlockingBreak,
  isRetriable,
  type Run,
  type RunStep,
  retryOf,
  runStatus,
  splitChildTasks,
} from './run';

const plan: RunStep = {
  seq: 1,
  entry: 'plan',
  visit: 1,
  startedAt: '2026-11-16T09:00:00Z',
  end: {
    outcome: 'done',
    output: 'Add a column.',
    endedAt: '2026-11-16T09:01:00Z',
  },
  gate: {
    kind: 'flow',
    question: 'Approve this plan?',
    answers: ['approve', 'revise'],
    openedAt: '2026-11-16T09:01:00Z',
  },
};

function run(steps: RunStep[], end?: Run['end']): Run {
  return {
    id: 'run-1',
    request: 'Add a due date column',
    createdAt: '2026-11-16T09:00:00Z',
    steps,
    end,
  };
}

describe('runStatus', () => {
  it('starts running before its first Step', () => {
    expect(runStatus(run([]))).toEqual({ kind: 'running', step: undefined });
  });

  it('waits on the open Gate of its latest Step', () => {
    expect(runStatus(run([plan]))).toEqual({
      kind: 'waiting',
      step: plan,
      gate: plan.gate,
    });
  });

  it('runs on once the Gate is answered', () => {
    const answered: RunStep = {
      ...plan,
      gate: {
        kind: 'flow',
        question: 'Approve this plan?',
        answers: ['approve', 'revise'],
        openedAt: '2026-11-16T09:01:00Z',
        answer: { answer: 'approve', answeredAt: '2026-11-16T09:02:00Z' },
      },
    };
    expect(runStatus(run([answered]))).toEqual({
      kind: 'running',
      step: answered,
    });
  });

  it('does not wait on an earlier Step once a later one started', () => {
    const write: RunStep = {
      seq: 2,
      entry: 'write',
      visit: 1,
      startedAt: '2026-11-16T09:03:00Z',
    };
    expect(runStatus(run([plan, write]))).toEqual({
      kind: 'running',
      step: write,
    });
  });

  it('has ended once the Run has an end, even at an open Gate', () => {
    expect(
      runStatus(
        run([plan], { outcome: 'cancelled', endedAt: '2026-11-16T09:05:00Z' })
      )
    ).toEqual({
      kind: 'ended',
      outcome: 'cancelled',
      endedAt: '2026-11-16T09:05:00Z',
    });
  });
});

describe('gateAsker', () => {
  it('names the Role of the agent that asks, and no one for a Flow’s Gate', () => {
    expect(gateAsker({ role: 'planner' }, { kind: 'question' })).toBe(
      'planner'
    );
    expect(gateAsker({}, { kind: 'question' })).toBe('agent');
    expect(gateAsker({ role: 'planner' }, { kind: 'flow' })).toBeUndefined();
  });
});

describe('retries', () => {
  const ended = (outcome: NonNullable<Run['end']>['outcome']): Run => ({
    id: 'run-1',
    request: 'Add a column',
    createdAt: '2026-11-16T09:00:00Z',
    flow: 'code',
    steps: [],
    end: { outcome, endedAt: '2026-11-16T10:00:00Z' },
  });

  it('retries a Run that ended failed, infrastructure or blocked, once', () => {
    for (const outcome of ['failed', 'infrastructure', 'blocked'] as const) {
      expect(isRetriable(ended(outcome), false)).toBe(true);
      expect(isRetriable(ended(outcome), true)).toBe(false);
    }
    for (const outcome of [
      'success',
      'manual',
      'cancelled',
      'flawed',
    ] as const) {
      expect(isRetriable(ended(outcome), false)).toBe(false);
    }
    expect(isRetriable({ ...ended('failed'), end: undefined }, false)).toBe(
      false
    );
  });

  it('leaves a Benchmark trial and a self-improvement Run to what started them', () => {
    expect(isRetriable({ ...ended('failed'), flow: 'benchmark' }, false)).toBe(
      false
    );
    expect(
      isRetriable({ ...ended('infrastructure'), flow: 'improve' }, false)
    ).toBe(false);
    expect(isRetriable({ ...ended('failed'), flow: undefined }, false)).toBe(
      true
    );
  });

  it('finds a Run’s retry among its task’s Runs by the Run it retries', () => {
    const retry: Run = {
      ...ended('failed'),
      id: 'run-2',
      retry: { retriedFrom: 'run-1', branchOf: 'run-1', firstSeq: 3 },
    };
    const other: Run = { ...ended('failed'), id: 'run-3' };
    expect(retryOf({ id: 'run-1' }, [other, retry])?.id).toBe('run-2');
    expect(retryOf({ id: 'run-2' }, [other, retry])).toBeUndefined();
  });

  it('takes the retry a Run names, listed or not', () => {
    expect(retryOf({ id: 'run-1', retriedBy: 'run-9' }, [])?.id).toBe('run-9');
  });
});

describe('splitChildTasks', () => {
  const A = '01a2077b-0000-7000-8000-00000000000a';
  const B = '01a2077b-0000-7000-8000-00000000000b';
  const PARENT = '01a2077b-0000-7000-8000-0000000000ff';
  const mention = (id: string, name: string) =>
    `<m-document-mention>${JSON.stringify({
      documentId: id,
      documentName: name,
      blockName: 'md',
      blockParams: {},
    })}</m-document-mention>`;
  // As the engine's split writes its output.
  const header = (why: string, count: number) =>
    `The task is too big for one run (${why}): its plan split it into ${count} child tasks, filed as the run's owner. The run ends here; start a run on each child task.`;
  const split = (output: string): RunStep => ({
    seq: 3,
    entry: 'split',
    visit: 1,
    startedAt: '2026-11-16T09:00:00Z',
    end: { outcome: 'done', output, endedAt: '2026-11-16T09:01:00Z' },
  });

  it('reads the child tasks a split filed, in order, from its list', () => {
    const output = [
      header('two services', 2),
      '',
      `- ${mention(A, 'Add the column')}`,
      `- ${mention(B, 'Show it in the list')}`,
      `- ${mention(A, 'Add the column')}`,
      '- <m-document-mention>{not json}</m-document-mention>',
    ].join('\n');

    expect(splitChildTasks(split(output))).toEqual([
      { id: A, name: 'Add the column' },
      { id: B, name: 'Show it in the list' },
    ]);
  });

  it('leaves out a task the plan’s reasons mention, ids that are no task’s, and the refused', () => {
    const output = [
      // The plan's reasons can list tasks of their own, on lines above
      // the factory's list.
      header(
        `it spans two services:\n- ${mention(PARENT, 'The parent')}\n- the list`,
        3
      ),
      '',
      `- ${mention(A, 'Add the column')}`,
      `- ${mention('task-b', 'Not an id')}`,
      '',
      'Document storage refused these, so file them by hand:',
      '',
      `- ${mention(B, 'Refused')}`,
    ].join('\n');

    expect(splitChildTasks(split(output))).toEqual([
      { id: A, name: 'Add the column' },
    ]);
  });

  it('reads the child tasks the factory recorded for the split, before its list', () => {
    const output = `${header('big', 1)}\n\n- ${mention(A, 'Add the column')}`;
    const recorded = [
      { seq: 3, id: B, name: 'Show it in the list' },
      { seq: 5, id: PARENT, name: 'Another split’s' },
    ];

    expect(splitChildTasks(split(output), recorded)).toEqual([
      { id: B, name: 'Show it in the list' },
    ]);
    expect(splitChildTasks(split(output), recorded.slice(1))).toEqual([
      { id: A, name: 'Add the column' },
    ]);
  });

  it('reads no child tasks from another Step, a split still running, or one with no list', () => {
    const output = `${header('big', 1)}\n\n- ${mention(A, 'Add the column')}`;
    expect(splitChildTasks({ ...split(output), entry: 'plan' })).toEqual([]);
    expect(splitChildTasks({ ...split(output), end: undefined })).toEqual([]);
    expect(
      splitChildTasks(split(`Filed ${mention(A, 'Add the column')}`))
    ).toEqual([]);
  });
});

describe('break records', () => {
  const record = (seq: number, status: BreakStatus): BreakRecord => ({
    seq,
    test: `test ${seq}`,
    file: 'src/due.ts',
    oldText: 'return date;',
    newText: 'return null;',
    status,
    output: '',
  });

  it('lists the breaks of one Step, in their order', () => {
    const records = [
      record(2, 'verified'),
      record(4, 'hung'),
      record(2, 'not_run'),
    ];
    expect(breakRecordsOf(records, 2).map((one) => one.status)).toEqual([
      'verified',
      'not_run',
    ]);
    expect(breakRecordsOf(records, 3)).toEqual([]);
  });

  it('blocks on a break that leaves its test unproven', () => {
    const blocking: BreakStatus[] = [
      'not_reproduced',
      'not_run',
      'not_applied',
      'unreadable',
      'hung',
    ];
    const others: BreakStatus[] = [
      'verified',
      'dropped',
      'unchecked',
      'interrupted',
    ];
    for (const status of blocking) expect(isBlockingBreak(status)).toBe(true);
    for (const status of others) expect(isBlockingBreak(status)).toBe(false);
  });
});
