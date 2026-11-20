import { describe, expect, it } from 'vitest';
import {
  durationIn,
  type EntryInsight,
  flawedRate,
  infrastructureRate,
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
  infrastructure: 0,
  flawed: 0,
  ...changes,
});

const entry = (
  name: string,
  failedSteps: number,
  failedRuns: number,
  revisitedRuns: number,
  troubledRuns: number
): EntryInsight => ({
  entry: name,
  steps: 9,
  runs: 4,
  revisitedRuns,
  failedRuns,
  troubledRuns,
  outcomes: failedSteps > 0 ? [{ name: 'failed', count: failedSteps }] : [],
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

  it('leaves infrastructure failures out, and counts flawed runs against it', () => {
    // Two failed for infrastructure are left out; one flawed counts: 3 of 5.
    const rate = successRate(
      counts({ success: 3, flawed: 1, failed: 1, infrastructure: 2 })
    );

    expect(rate).toBe(0.6);
  });
});

describe('outcome kinds', () => {
  it('measures flawed runs among those that reached their end', () => {
    expect(flawedRate(counts({ success: 3, flawed: 1, failed: 4 }))).toBe(0.25);
    expect(flawedRate(counts({ failed: 2 }))).toBeUndefined();
  });

  it('measures infrastructure failures among runs that ended and were not cancelled', () => {
    // The cancelled run is left out: 1 of 4.
    const rate = infrastructureRate(
      counts({ success: 2, failed: 1, infrastructure: 1, cancelled: 1 })
    );

    expect(rate).toBe(0.25);
    expect(infrastructureRate(counts({ running: 1 }))).toBeUndefined();
  });
});

describe('troubledEntries', () => {
  it('lists the entries where runs failed or went again, worst first', () => {
    // CI failed five times in one run that went back too: one troubled
    // run. Review has two failed runs, one of which went back: two. Fix had
    // three runs go back: three. Ties keep their order.
    const entries = [
      entry('plan', 0, 0, 1, 1),
      entry('write', 0, 0, 0, 0),
      entry('ci', 5, 1, 1, 1),
      entry('review', 2, 2, 1, 2),
      entry('fix', 0, 0, 3, 3),
    ];

    expect(troubledEntries(entries).map((found) => found.entry)).toEqual([
      'fix',
      'review',
      'plan',
      'ci',
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
