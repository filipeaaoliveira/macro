import { describe, expect, it } from 'vitest';
import {
  durationIn,
  type EntryInsight,
  insightsParams,
  type OutcomeCounts,
  successRate,
  troubledEntries,
} from './insights';

const counts = (changes: Partial<OutcomeCounts>): OutcomeCounts => ({
  running: 0,
  success: 0,
  manual: 0,
  blocked: 0,
  failed: 0,
  cancelled: 0,
  ...changes,
});

const entry = (
  name: string,
  failed: number,
  revisitedRuns: number
): EntryInsight => ({
  entry: name,
  steps: 4,
  runs: 4,
  revisitedRuns,
  outcomes: failed > 0 ? [{ name: 'failed', count: failed }] : [],
  trouble: [],
});

describe('successRate', () => {
  it('counts only the runs that ended on their own', () => {
    // Two cancelled and three still going are left out: 3 of 4.
    const rate = successRate(
      counts({ success: 3, failed: 1, cancelled: 2, running: 3 })
    );

    expect(rate).toBe(0.75);
  });

  it('has no rate while no run ended on its own', () => {
    expect(successRate(counts({ running: 2, cancelled: 1 }))).toBeUndefined();
  });
});

describe('troubledEntries', () => {
  it('lists the entries that failed or went again, worst first', () => {
    const entries = [
      entry('plan', 0, 1),
      entry('write', 0, 0),
      entry('ci', 2, 1),
      entry('review', 1, 0),
    ];

    expect(troubledEntries(entries).map((found) => found.entry)).toEqual([
      'ci',
      'plan',
      'review',
    ]);
  });
});

describe('durationIn', () => {
  it('reads each duration in the unit that suits it', () => {
    expect(durationIn(45)).toEqual({ value: 45, unit: 'second' });
    expect(durationIn(600)).toEqual({ value: 10, unit: 'minute' });
    expect(durationIn(3 * 3600)).toEqual({ value: 3, unit: 'hour' });
    expect(durationIn(3 * 86_400)).toEqual({ value: 3, unit: 'day' });
  });
});

describe('insightsParams', () => {
  it('reads the last days up to today in the person’s time zone', () => {
    // 23:30 UTC is already the next day in Lisbon's summer.
    const now = new Date('2026-07-10T23:30:00Z');

    expect(insightsParams(7, 'Europe/Lisbon', 'plan', now)).toEqual({
      from: '2026-07-05',
      to: '2026-07-11',
      timeZone: 'Europe/Lisbon',
      flow: 'plan',
    });
  });
});
