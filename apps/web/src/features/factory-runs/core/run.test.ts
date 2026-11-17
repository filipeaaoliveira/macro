import { describe, expect, it } from 'vitest';
import { gateAsker, type Run, type RunStep, runStatus } from './run';

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
