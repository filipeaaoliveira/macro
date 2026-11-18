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
  'flawed',
  'manual',
  'blocked',
  'failed',
  'infrastructure',
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
 * The share of Runs that reached their Flow's end with a result no one
 * found wrong, among those whose work ended on its own: Runs still going,
 * Runs a person cancelled and Runs that failed for infrastructure are left
 * out, so the rate measures the factory's work, not its plumbing. A flawed
 * Run counts against it. None while no Run ended that way.
 */
export function successRate(counts: OutcomeCounts): number | undefined {
  const ended =
    counts.success +
    counts.flawed +
    counts.manual +
    counts.blocked +
    counts.failed;
  if (ended === 0) return undefined;
  return counts.success / ended;
}

/**
 * Of the Runs that ended on their own, the share that failed for
 * infrastructure: a machine, quota or network reason. None while no Run
 * ended that way.
 */
export function infrastructureRate(counts: OutcomeCounts): number | undefined {
  const ended =
    counts.success +
    counts.flawed +
    counts.manual +
    counts.blocked +
    counts.failed +
    counts.infrastructure;
  if (ended === 0) return undefined;
  return counts.infrastructure / ended;
}

/**
 * Of the Runs that reached their Flow's end, the share whose result a
 * Scorer or a person found wrong afterwards. None while none succeeded.
 */
export function flawedRate(counts: OutcomeCounts): number | undefined {
  const finished = counts.success + counts.flawed;
  if (finished === 0) return undefined;
  return counts.flawed / finished;
}

/**
 * The entries where Runs stop or go round again, worst first: by the Runs
 * in which one of its Steps failed or that visited it more than once, each
 * Run counted once. Entries with none are left out.
 */
export function troubledEntries(entries: EntryInsight[]): EntryInsight[] {
  return entries
    .filter((entry) => entry.troubledRuns > 0)
    .sort((a, b) => b.troubledRuns - a.troubledRuns);
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
