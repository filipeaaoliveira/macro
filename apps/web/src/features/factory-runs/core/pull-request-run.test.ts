import { describe, expect, it } from 'vitest';
import { type PullRequestRun, pullRequestMerge } from './pull-request-run';

const JULIA = 'macro|julia@example.com';
const TEO = 'macro|teo@example.com';

const atGate: PullRequestRun = {
  id: 'run-1',
  state: 'waiting',
  mergeGateSeq: 7,
  run: {
    id: 'run-1',
    request: 'Add the coupon field',
    createdAt: '2026-11-18T10:00:00Z',
    ownerId: JULIA,
    repository: { url: 'https://github.com/acme/shop', baseBranch: 'main' },
    pullRequestUrl: 'https://github.com/acme/shop/pull/7',
    steps: [
      { seq: 6, entry: 'ci', visit: 1, startedAt: '2026-11-18T10:20:00Z' },
      {
        seq: 7,
        entry: 'merge-gate',
        visit: 1,
        startedAt: '2026-11-18T10:30:00Z',
      },
    ],
  },
};

describe('pullRequestMerge', () => {
  it('merges nothing until it is known whether a Run opened it', () => {
    expect(pullRequestMerge({ kind: 'unknown' }, JULIA)).toEqual({
      kind: 'refused',
      why: 'checking',
    });
  });

  it('leaves a pull request no Run opened to GitHub', () => {
    expect(pullRequestMerge({ kind: 'none' }, JULIA)).toEqual({
      kind: 'github',
    });
  });

  it('leaves a pull request to GitHub once its Run ended, saying it stopped', () => {
    const ended: PullRequestRun = {
      ...atGate,
      state: 'stopped',
      mergeGateSeq: undefined,
      run: {
        ...atGate.run,
        end: { outcome: 'manual', endedAt: '2026-11-18T11:00:00Z' },
      },
    };
    expect(pullRequestMerge({ kind: 'run', found: ended }, JULIA)).toEqual({
      kind: 'github',
      stopped: 'manual',
    });
  });

  it('merges through the merge Gate its owner is asked', () => {
    expect(pullRequestMerge({ kind: 'run', found: atGate }, JULIA)).toEqual({
      kind: 'gate',
      runId: 'run-1',
    });
  });

  it('refuses anyone but the owner, even at the merge Gate', () => {
    expect(pullRequestMerge({ kind: 'run', found: atGate }, TEO)).toEqual({
      kind: 'refused',
      why: 'notOwner',
      ownerId: JULIA,
    });
    expect(
      pullRequestMerge({ kind: 'run', found: atGate }, undefined)
    ).toMatchObject({
      kind: 'refused',
      why: 'notOwner',
    });
  });

  it('refuses while a Main Hold holds the base', () => {
    const held: PullRequestRun = {
      ...atGate,
      state: 'held',
      mergeGateSeq: undefined,
    };
    expect(pullRequestMerge({ kind: 'run', found: held }, JULIA)).toEqual({
      kind: 'refused',
      why: 'held',
      branch: 'main',
    });
  });

  it('refuses before the Run reaches its merge Gate, naming its Step', () => {
    const reviewing: PullRequestRun = {
      ...atGate,
      state: 'running',
      mergeGateSeq: undefined,
      run: {
        ...atGate.run,
        steps: [
          {
            seq: 3,
            entry: 'review',
            visit: 2,
            startedAt: '2026-11-18T10:10:00Z',
          },
        ],
      },
    };
    expect(pullRequestMerge({ kind: 'run', found: reviewing }, JULIA)).toEqual({
      kind: 'refused',
      why: 'notAtGate',
      step: reviewing.run.steps[0],
      waiting: false,
    });
  });

  it('says the Run waits at another Gate before its merge Gate', () => {
    const asking: PullRequestRun = {
      ...atGate,
      mergeGateSeq: undefined,
      run: {
        ...atGate.run,
        steps: [
          {
            seq: 2,
            entry: 'plan',
            visit: 1,
            startedAt: '2026-11-18T10:05:00Z',
            gate: {
              kind: 'flow',
              question: 'Approve the plan?',
              answers: ['approve', 'revise'],
              openedAt: '2026-11-18T10:06:00Z',
            },
          },
        ],
      },
    };
    expect(pullRequestMerge({ kind: 'run', found: asking }, JULIA)).toEqual({
      kind: 'refused',
      why: 'notAtGate',
      step: asking.run.steps[0],
      waiting: true,
    });
  });
});
