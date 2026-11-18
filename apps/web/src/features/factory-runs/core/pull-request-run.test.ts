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
  it('leaves a pull request no Run opened to GitHub', () => {
    expect(pullRequestMerge(undefined, JULIA)).toEqual({ kind: 'github' });
  });

  it('leaves a pull request to GitHub once its Run ended', () => {
    const ended: PullRequestRun = {
      ...atGate,
      state: 'stopped',
      mergeGateSeq: undefined,
      run: {
        ...atGate.run,
        end: { outcome: 'manual', endedAt: '2026-11-18T11:00:00Z' },
      },
    };
    expect(pullRequestMerge(ended, JULIA)).toEqual({ kind: 'github' });
  });

  it('merges through the merge Gate its owner is asked', () => {
    expect(pullRequestMerge(atGate, JULIA)).toEqual({
      kind: 'gate',
      runId: 'run-1',
    });
  });

  it('refuses anyone but the owner, even at the merge Gate', () => {
    expect(pullRequestMerge(atGate, TEO)).toEqual({
      kind: 'refused',
      why: 'notOwner',
      ownerId: JULIA,
    });
    expect(pullRequestMerge(atGate, undefined)).toMatchObject({
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
    expect(pullRequestMerge(held, JULIA)).toEqual({
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
    expect(pullRequestMerge(reviewing, JULIA)).toEqual({
      kind: 'refused',
      why: 'notAtGate',
      step: reviewing.run.steps[0],
    });
  });
});
