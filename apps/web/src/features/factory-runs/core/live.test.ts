import { describe, expect, it } from 'vitest';
import type { BoardRun } from './board';
import { liveNumbers, mergedAt, stepRuntime } from './live';
import type { Run } from './run';

/**
 * A time on a day of November 2026, on the local clock: "today" is the
 * local day, so the test reads the same in every time zone.
 */
function at(day: number, hour: number, minute = 0): string {
  return new Date(2026, 10, day, hour, minute).toISOString();
}

const NOW = Date.parse(at(24, 12));

function merged(
  id: string,
  times: { created: string; end: string; merge?: string },
  outcome: 'success' | 'flawed' = 'success'
): BoardRun {
  return {
    id,
    state: 'finished',
    run: {
      id,
      request: 'Add the coupon field',
      createdAt: times.created,
      steps: [],
      end: { outcome, endedAt: times.end },
      mergedAt: times.merge,
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
        // Merged yesterday, ended today: not merged today. 3h to merge.
        merged('run-1', {
          created: at(23, 9),
          merge: at(23, 12),
          end: at(24, 10, 30),
        }),
        // From before merge times: its end stands in. 2h to merge.
        merged('run-2', { created: at(24, 9), end: at(24, 11) }),
        // 1h 30m to merge.
        merged('run-3', {
          created: at(24, 9),
          merge: at(24, 10, 30),
          end: at(24, 11, 30),
        }),
        // Merged and found wrong after: merged, but no success. 1h.
        merged(
          'run-4',
          { created: at(24, 9), merge: at(24, 10), end: at(24, 10, 15) },
          'flawed'
        ),
      ],
      now: NOW,
    });

    expect(numbers.mergedToday).toBe(3);
    // 1h, 1h 30m, 2h and 3h: the median is 1h 30m.
    expect(numbers.medianToMerge).toBe(90 * 60 * 1000);
    expect(numbers.successRate).toBe(75);
  });
});
