import { insightPresetRange } from '@app/features/scheduling/core/insights';
import type {
  EntryInsight,
  OutcomeCounts,
} from '@service-factory/generated/schemas';

export type {
  DurationStats,
  EntryInsight,
  FlowInsight,
  InsightDay,
  OutcomeCounts,
  RunInsights,
  RunLink,
  TokenCounts,
  TroubleRun,
} from '@service-factory/generated/schemas';

/** A Run's state in insights: how it ended, or `running` while it goes. */
export type InsightOutcome = keyof OutcomeCounts;

/** Every state, in the order charts stack and legends list them. */
export const INSIGHT_OUTCOMES: readonly InsightOutcome[] = [
  'success',
  'manual',
  'blocked',
  'failed',
  'cancelled',
  'running',
];

/** The periods a person picks from, in days back from today. */
export const INSIGHT_PERIODS = [7, 30, 90] as const;
export type InsightPeriod = (typeof INSIGHT_PERIODS)[number];

/** What insights are read for. */
export type InsightsParams = {
  from: string;
  to: string;
  timeZone: string;
  /** A Flow's name, or none for every Flow. */
  flow?: string;
};

/** The last `days` days, today included, in `timeZone`. */
export function insightsParams(
  days: InsightPeriod,
  timeZone: string,
  flow: string | undefined,
  now: Date
): InsightsParams {
  const range = insightPresetRange(days, timeZone, now);
  return { ...range, timeZone, flow };
}

export function runCount(counts: OutcomeCounts): number {
  return INSIGHT_OUTCOMES.reduce((sum, outcome) => sum + counts[outcome], 0);
}

/**
 * The share of Runs that reached their Flow's end, among those that ended
 * on their own: Runs still going and Runs a person cancelled are left out.
 * None while no Run ended that way.
 */
export function successRate(counts: OutcomeCounts): number | undefined {
  const ended = counts.success + counts.manual + counts.blocked + counts.failed;
  if (ended === 0) return undefined;
  return counts.success / ended;
}

/**
 * The entries where Runs stop or go round again, worst first: the Runs in
 * which one of its Steps failed, plus the Runs that visited it more than
 * once. Entries with neither are left out.
 */
export function troubledEntries(entries: EntryInsight[]): EntryInsight[] {
  const score = (entry: EntryInsight) => entry.failedRuns + entry.revisitedRuns;
  return entries
    .filter((entry) => score(entry) > 0)
    .sort((a, b) => score(b) - score(a));
}

/** The unit a duration reads best in, and its value in that unit. */
export type DurationUnit = 'second' | 'minute' | 'hour' | 'day';

export function durationIn(seconds: number): {
  value: number;
  unit: DurationUnit;
} {
  if (seconds < 90) return { value: seconds, unit: 'second' };
  if (seconds < 90 * 60) return { value: seconds / 60, unit: 'minute' };
  if (seconds < 48 * 3600) return { value: seconds / 3600, unit: 'hour' };
  return { value: seconds / 86_400, unit: 'day' };
}

/** All the tokens of a count, cache reads and writes included. */
export function tokenTotal(tokens: {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}): number {
  return tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite;
}
