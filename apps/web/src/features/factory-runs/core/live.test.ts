import { describe, expect, it } from 'vitest';
import type { BoardRun } from './board';
import { liveNumbers, mergedAt, stepRuntime } from './live';
import type { Run } from './run';

const NOW = Date.parse('2026-11-24T12:00:00Z');

function merged(id: string, end: string, merge?: string): BoardRun {
  return {
    id,
    state: 'finished',
    run: {
      id,
      request: 'Add the coupon field',
      createdAt: '2026-11-24T09:00:00Z',
      steps: [],
      end: { outcome: 'success', endedAt: end },
      mergedAt: merge,
    },
  };
}

describe('stepRuntime', () => {
  it('reads where a Step ran, and nothing for a Step that has no runtime', () => {
    expect(
      stepRuntime({
        runtime: {
          model: 'claude-opus-4-5',
          machine: 'm4',
          agentName: 'Writer',
        },
      })
    ).toEqual({ model: 'claude-opus-4-5', machine: 'm4', agentName: 'Writer' });
    expect(stepRuntime({})).toEqual({
      model: undefined,
      machine: undefined,
      agentName: undefined,
    });
    expect(stepRuntime({ runtime: { model: '' } }).model).toBeUndefined();
  });
});

describe('mergedAt', () => {
  const end = { outcome: 'success' as const, endedAt: '2026-11-24T12:00:00Z' };

  it('is the recorded merge time, else a merged Run’s end, else none', () => {
    expect(mergedAt({ end, mergedAt: '2026-11-24T11:00:00Z' })).toBe(
      '2026-11-24T11:00:00Z'
    );
    expect(mergedAt({ end })).toBe('2026-11-24T12:00:00Z');
    expect(
      mergedAt({ end: { outcome: 'failed', endedAt: end.endedAt } })
    ).toBeUndefined();
    expect(mergedAt({} as Pick<Run, 'mergedAt' | 'end'>)).toBeUndefined();
  });
});

describe('liveNumbers', () => {
  it('counts a Run merged today, and its time to merge, by its merge', () => {
    const numbers = liveNumbers({
      active: [],
      waiting: 0,
      recent: [
        // Merged yesterday, ended today: not merged today.
        merged('run-1', '2026-11-24T10:30:00Z', '2026-11-23T12:00:00Z'),
        // From before merge times: its end stands in.
        merged('run-2', '2026-11-24T11:00:00Z'),
        merged('run-3', '2026-11-24T11:30:00Z', '2026-11-24T10:30:00Z'),
      ],
      now: NOW,
    });

    expect(numbers.mergedToday).toBe(2);
    // To merge: before it started (none), 2h and 1h 30m; the median is 1h 30m.
    expect(numbers.medianToMerge).toBe(90 * 60 * 1000);
  });
});
