import { describe, expect, it } from 'vitest';
import { boardPollMs, flattenBoardPages } from '../queries/board';
import { currentStep, openBlockers, startedBy } from './board';
import type { ReviewRound, Run } from './run';

const run: Run = {
  id: 'run-1',
  request: 'Add the coupon field',
  createdAt: '2026-11-18T10:00:00Z',
  steps: [
    { seq: 1, entry: 'plan', visit: 1, startedAt: '2026-11-18T10:00:00Z' },
    { seq: 2, entry: 'write', visit: 1, startedAt: '2026-11-18T10:05:00Z' },
  ],
};

const round = (
  outcome: 'pass' | 'changes' | undefined,
  blockers: number
): ReviewRound => ({
  round: 1,
  visit: 1,
  seq: 3,
  reviewers: 1,
  fixDiff: false,
  startedAt: '2026-11-18T10:10:00Z',
  end: outcome ? { outcome, endedAt: '2026-11-18T10:11:00Z' } : undefined,
  findings: Array.from({ length: blockers }, (_, place) => ({
    seq: 3,
    place,
    path: 'src/cart.ts',
    problem: 'Wrong total',
    why: 'Customers pay less',
    blocksAnywhere: false,
    kind: 'blocker' as const,
  })),
});

describe('startedBy', () => {
  it('names an Automation, the owner in person, or an agent for them', () => {
    expect(
      startedBy({
        automation: { id: 'a', name: 'Nightly', trigger: 'schedule' },
        createdInPerson: false,
      })
    ).toBe('automation');
    expect(startedBy({ createdInPerson: true })).toBe('person');
    expect(startedBy({ createdInPerson: false })).toBe('orchestrator');
  });
});

describe('currentStep', () => {
  it('is the latest Step while the Run goes, and none once it ended', () => {
    expect(currentStep(run)?.entry).toBe('write');
    expect(
      currentStep({
        ...run,
        end: { outcome: 'success', endedAt: '2026-11-18T11:00:00Z' },
      })
    ).toBeUndefined();
  });
});

describe('openBlockers', () => {
  it('counts the latest round’s Blockers once it asked for changes', () => {
    expect(openBlockers({ ...run, reviewRounds: [round('changes', 2)] })).toBe(
      2
    );
    // A later round that passed reviewed the fix: none are open.
    expect(
      openBlockers({
        ...run,
        reviewRounds: [round('changes', 2), round('pass', 0)],
      })
    ).toBe(0);
    // A round still reviewing has found nothing yet.
    expect(openBlockers({ ...run, reviewRounds: [round(undefined, 1)] })).toBe(
      0
    );
    expect(
      openBlockers({
        ...run,
        reviewRounds: [round('changes', 1)],
        end: { outcome: 'failed', endedAt: '2026-11-18T11:00:00Z' },
      })
    ).toBe(0);
  });
});

describe('the board’s pages', () => {
  it('lists each Run once, in page order', () => {
    const second = { ...run, id: 'run-2' };
    expect(
      flattenBoardPages([
        { runs: [{ id: run.id, state: 'running', run }] },
        {
          runs: [
            { id: run.id, state: 'running', run },
            { id: second.id, state: 'waiting', run: second },
          ],
        },
      ]).map((listed) => listed.run.id)
    ).toEqual(['run-1', 'run-2']);
  });

  it('is read fast while a Run goes, and slowly once all ended', () => {
    const ended = {
      ...run,
      end: { outcome: 'success' as const, endedAt: '2026-11-18T11:00:00Z' },
    };
    expect(boardPollMs([{ id: run.id, state: 'running', run }])).toBe(15_000);
    expect(boardPollMs([{ id: ended.id, state: 'finished', run: ended }])).toBe(
      60_000
    );
  });
});
