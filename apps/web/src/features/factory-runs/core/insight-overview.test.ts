import type { DeliveryInsightWithMergeTime } from '@service-factory/pending';
import { describe, expect, it } from 'vitest';
import {
  dayMedianRunSeconds,
  medianMergeSeconds,
  roleInsights,
} from './insight-overview';
import type { EntryInsight, OutcomeCounts } from './insights';

const counts: OutcomeCounts = {
  running: 0,
  success: 1,
  manual: 0,
  blocked: 0,
  failed: 0,
  cancelled: 0,
  infrastructure: 0,
  flawed: 0,
  retried: 0,
};

const entry = (
  name: string,
  tokens: number | undefined,
  costUsd: number | null | undefined
): EntryInsight & { costUsd?: number | null } => ({
  entry: name,
  steps: 2,
  runs: 2,
  revisitedRuns: 0,
  failedRuns: 0,
  troubledRuns: 0,
  outcomes: [{ name: 'done', count: 2 }],
  trouble: [],
  tokens:
    tokens === undefined
      ? undefined
      : { input: tokens, output: 0, cacheRead: 0, cacheWrite: 0 },
  costUsd,
});

const delivery = (
  timeToMerge: DeliveryInsightWithMergeTime['timeToMerge']
): DeliveryInsightWithMergeTime => ({
  endedRuns: 2,
  merged: 2,
  autonomous: 2,
  personAnswered: 0,
  steered: 0,
  manual: 0,
  measuredRuns: 0,
  unmeasuredRuns: 0,
  tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  costUsd: 0,
  unpricedCalls: 0,
  timeToMerge,
});

describe('dayMedianRunSeconds', () => {
  it('reads a day’s median run time, and none for a day with none', () => {
    expect(
      dayMedianRunSeconds({
        date: '2026-11-20',
        counts,
        runDuration: { count: 3, medianSeconds: 600, p90Seconds: 900 },
      })
    ).toBe(600);
    expect(
      dayMedianRunSeconds({ date: '2026-11-21', counts, runDuration: null })
    ).toBeUndefined();
    expect(dayMedianRunSeconds({ date: '2026-11-22', counts })).toBeUndefined();
  });
});

describe('medianMergeSeconds', () => {
  it('reads the median time to merge, and none before a merge', () => {
    expect(
      medianMergeSeconds(
        delivery({ count: 2, medianSeconds: 5400, p90Seconds: 7000 })
      )
    ).toBe(5400);
    expect(medianMergeSeconds(delivery(null))).toBeUndefined();
    expect(medianMergeSeconds(undefined)).toBeUndefined();
  });
});

describe('roleInsights', () => {
  it('costs a Role in money a Run when every entry with usage was priced', () => {
    const [writer] = roleInsights([
      entry('write', 1000, 1.5),
      entry('update', 500, 0.5),
    ]);

    expect(writer?.role).toBe('writer');
    expect(writer?.usdPerRun).toBe(1);
    expect(writer?.tokensPerRun).toBe(750);
  });

  it('falls back to tokens when an entry with usage was not priced in full', () => {
    const [writer] = roleInsights([
      entry('write', 1000, 1.5),
      entry('update', 500, null),
    ]);

    expect(writer?.usdPerRun).toBeUndefined();
    expect(writer?.tokensPerRun).toBe(750);
  });

  it('gives no cost to a Role whose usage was not measured', () => {
    const [writer] = roleInsights([entry('write', undefined, undefined)]);

    expect(writer?.usdPerRun).toBeUndefined();
    expect(writer?.tokensPerRun).toBeUndefined();
  });
});
