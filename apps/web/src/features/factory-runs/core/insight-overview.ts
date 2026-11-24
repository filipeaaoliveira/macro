import { shiftReportDate } from '@app/features/scheduling/core/insights';
import type { BlockerCount } from '@service-factory/generated/schemas';
import type {
  DeliveryInsightWithMergeTime,
  EntryInsightWithCost,
  InsightDayWithDuration,
} from '@service-factory/pending';
import type { SuiteInsight } from './evaluation';
import { fellShort } from './flow-words';
import {
  type EntryInsight,
  type InsightDay,
  type InsightsParams,
  type OutcomeCounts,
  type RunInsights,
  runCount,
  successRate,
  tokenTotal,
} from './insights';
import {
  type DeliveryInsight,
  mergedCost,
  type QualityInsights,
} from './quality';

/**
 * What the insights overview reads from the period's insights, its quality
 * and evaluation, and the same of the period before it. Fields the
 * generated client does not have yet are read through the hand-written
 * types of `@service-factory/pending`.
 */

/** The period of the same length just before `params`'. */
export function previousParams(params: InsightsParams): InsightsParams {
  const days =
    Math.round(
      (Date.parse(`${params.to}T00:00:00Z`) -
        Date.parse(`${params.from}T00:00:00Z`)) /
        86_400_000
    ) + 1;
  return {
    ...params,
    from: shiftReportDate(params.from, -days),
    to: shiftReportDate(params.from, -1),
  };
}

/**
 * How the Runs ended, in the dashboard's five tones: merged, failed (for
 * the work or the machines, or found wrong after), handed off to a person,
 * cancelled, and running.
 */
export type OutcomeGroup =
  | 'merged'
  | 'failed'
  | 'handedOff'
  | 'cancelled'
  | 'running';

/** Bottom to top as the chart stacks them, merged on top; legends list them reversed. */
export const OUTCOME_GROUPS: readonly OutcomeGroup[] = [
  'cancelled',
  'failed',
  'handedOff',
  'running',
  'merged',
];

/** The status colour each group takes, as on the dashboard. */
export const OUTCOME_GROUP_COLORS: Record<OutcomeGroup, string> = {
  merged: 'var(--color-success)',
  failed: 'var(--color-failure)',
  handedOff: 'var(--color-warning)',
  cancelled: 'var(--color-ink-disabled)',
  running: 'var(--color-accent)',
};

export function groupCounts(
  counts: OutcomeCounts
): Record<OutcomeGroup, number> {
  return {
    merged: counts.success,
    failed:
      counts.failed + counts.blocked + counts.infrastructure + counts.flawed,
    handedOff: counts.manual,
    cancelled: counts.cancelled,
    running: counts.running,
  };
}

/** A headline figure, its change from the period before, and its days. */
export type Kpi = {
  value: number;
  /** The same figure for the period before; none when it had none. */
  previous?: number;
  /** One value a day, oldest first; none where no day has one. */
  trend?: (number | undefined)[];
};

/** A day's median run time; none when none of its Runs ended timed. */
export function dayMedianRunSeconds(
  day: InsightDayWithDuration
): number | undefined {
  return day.runDuration?.medianSeconds ?? undefined;
}

/** The median time from a Run's start to its merge; none before a merge. */
export function medianMergeSeconds(
  delivery: DeliveryInsightWithMergeTime | undefined
): number | undefined {
  return delivery?.timeToMerge?.medianSeconds ?? undefined;
}

/** The days from the first that has a Run on; all of them when none has. */
export function daysWithData(days: InsightDay[]): InsightDay[] {
  const first = days.findIndex((day) => runCount(day.counts) > 0);
  return first <= 0 ? days : days.slice(first);
}

function dailyRate(day: InsightDay): number | undefined {
  const rate = successRate(day.counts);
  return rate === undefined ? undefined : rate * 100;
}

/** The headline figures; a figure no Run gave is left out. */
export type Kpis = {
  runs: Kpi;
  merged?: Kpi;
  successRate?: Kpi;
  medianRunSeconds?: Kpi;
  /** From a Run's start to its merge, median. */
  timeToMergeSeconds?: Kpi;
  autonomy?: Kpi;
  costPerMerge?: Kpi & { kind: 'money' | 'tokens' };
};

function percentOf(share: number | null | undefined): number | undefined {
  return share === null || share === undefined ? undefined : share * 100;
}

function cost(delivery: DeliveryInsight | undefined) {
  return delivery ? mergedCost(delivery) : undefined;
}

export function insightKpis(input: {
  insights: RunInsights;
  previous?: RunInsights;
  quality?: QualityInsights;
  previousQuality?: QualityInsights;
}): Kpis {
  const { insights, previous, quality, previousQuality } = input;
  const rate = successRate(insights.totals);
  const previousRate = previous ? successRate(previous.totals) : undefined;
  const autonomy = percentOf(quality?.delivery.autonomyRate);
  const merge = cost(quality?.delivery);
  const previousMerge = cost(previousQuality?.delivery);
  const toMerge = medianMergeSeconds(quality?.delivery);
  return {
    runs: {
      value: runCount(insights.totals),
      previous: previous ? runCount(previous.totals) : undefined,
      trend: insights.days.map((day) => runCount(day.counts)),
    },
    merged:
      insights.totals.success > 0 || (previous?.totals.success ?? 0) > 0
        ? {
            value: insights.totals.success,
            previous: previous?.totals.success,
            trend: insights.days.map((day) => day.counts.success),
          }
        : undefined,
    successRate:
      rate === undefined
        ? undefined
        : {
            value: rate * 100,
            previous:
              previousRate === undefined ? undefined : previousRate * 100,
            trend: insights.days.map(dailyRate),
          },
    medianRunSeconds: insights.runDuration
      ? {
          value: insights.runDuration.medianSeconds,
          previous: previous?.runDuration?.medianSeconds,
          trend: insights.days.some(
            (day) => dayMedianRunSeconds(day) !== undefined
          )
            ? insights.days.map(dayMedianRunSeconds)
            : undefined,
        }
      : undefined,
    timeToMergeSeconds:
      toMerge === undefined
        ? undefined
        : {
            value: toMerge,
            previous: medianMergeSeconds(previousQuality?.delivery),
          },
    autonomy:
      autonomy === undefined
        ? undefined
        : {
            value: autonomy,
            previous: percentOf(previousQuality?.delivery.autonomyRate),
          },
    costPerMerge: merge
      ? {
          kind: merge.kind,
          value: merge.kind === 'money' ? merge.usd : merge.tokens,
          previous:
            previousMerge && previousMerge.kind === merge.kind
              ? previousMerge.kind === 'money'
                ? previousMerge.usd
                : previousMerge.tokens
              : undefined,
        }
      : undefined,
  };
}

/**
 * Where the Runs' time goes: each entry's median time a visit and its
 * visits, in the Flow's order, entries no Step took time at left out.
 */
export type StepTime = {
  entry: string;
  medianSeconds: number;
  p90Seconds?: number;
  visits: number;
  runs: number;
  /** The entry's share of the period's Step time, by median × visits. */
  share: number;
  /** How long a person took to answer its Gate, median. */
  waitSeconds?: number;
  /** Whether an agent works it, or the factory alone. */
  agent: boolean;
};

export function stepTimes(entries: EntryInsight[]): StepTime[] {
  // An entry visited once in the period is noise beside the Flow's own.
  const timed = entries.filter(
    (entry) =>
      entry.duration && entry.duration.medianSeconds > 0 && entry.steps > 1
  );
  const total = timed.reduce(
    (sum, entry) => sum + (entry.duration?.medianSeconds ?? 0) * entry.steps,
    0
  );
  return timed.map((entry) => ({
    entry: entry.entry,
    medianSeconds: entry.duration?.medianSeconds ?? 0,
    p90Seconds: entry.duration?.p90Seconds ?? undefined,
    visits: entry.steps,
    runs: entry.runs,
    share:
      total > 0
        ? ((entry.duration?.medianSeconds ?? 0) * entry.steps) / total
        : 0,
    waitSeconds: entry.gate?.wait?.medianSeconds ?? undefined,
    agent: entry.entry in ENTRY_ROLES,
  }));
}

/**
 * The Role that works an entry of the built-in Flows. A Definition names
 * its own Roles; the API does not send an entry's Role yet, so this map
 * stands in for it.
 */
const ENTRY_ROLES: Record<string, string> = {
  plan: 'planner',
  'plan-review': 'plan-reviewer',
  write: 'writer',
  update: 'writer',
  review: 'reviewer',
  fix: 'fixer',
  'ci-fix': 'fixer',
  accept: 'verifier',
  verify: 'verifier',
};

/** One Role's period: how often its Steps passed, its rounds, its cost. */
export type RoleInsight = {
  role: string;
  entries: string[];
  steps: number;
  runs: number;
  /** Of its Steps that ended, the share that did not fall short. */
  passRate?: number;
  /** Visits a Run, on average: more than one means it went round again. */
  roundsPerRun: number;
  /** Its tokens a Run; none while its usage is not measured. */
  tokensPerRun?: number;
  /**
   * What it cost a Run, in US dollars; none unless each of its entries with
   * usage was priced in full.
   */
  usdPerRun?: number;
  /** For a reviewer, the blockers it raised a review. */
  blockersPerReview?: number;
  medianSeconds?: number;
};

/** Outcomes that are no verdict on the Step's work. */
const NEUTRAL_OUTCOMES = new Set(['cancelled', 'no_result', 'not_needed']);

export function roleInsights(
  entries: EntryInsightWithCost[],
  reviewers: BlockerCount[] = []
): RoleInsight[] {
  const byRole = new Map<string, EntryInsightWithCost[]>();
  for (const entry of entries) {
    const role = ENTRY_ROLES[entry.entry];
    if (!role) continue;
    byRole.set(role, [...(byRole.get(role) ?? []), entry]);
  }
  const reviews = reviewers.reduce((sum, row) => sum + row.reviews, 0);
  const blockers = reviewers.reduce((sum, row) => sum + row.blockers, 0);
  return [...byRole.entries()].map(([role, list]) => {
    let judged = 0;
    let passed = 0;
    for (const entry of list) {
      for (const outcome of entry.outcomes) {
        if (NEUTRAL_OUTCOMES.has(outcome.name)) continue;
        judged += outcome.count;
        // A review that asks for changes did its job; the change fell short.
        if (!fellShort(outcome.name) && outcome.name !== 'blockers') {
          passed += outcome.count;
        }
      }
    }
    const steps = list.reduce((sum, entry) => sum + entry.steps, 0);
    const runs = Math.max(...list.map((entry) => entry.runs), 0);
    const tokens = list.reduce(
      (sum, entry) => sum + (entry.tokens ? tokenTotal(entry.tokens) : 0),
      0
    );
    // Money only when every entry that used tokens was priced in full: a
    // sum that left one out would read its cost as nothing.
    const measured = list.filter((entry) => entry.tokens);
    const priced = measured.every(
      (entry) => entry.costUsd !== undefined && entry.costUsd !== null
    );
    const usd = measured.reduce((sum, entry) => sum + (entry.costUsd ?? 0), 0);
    const main = list[0];
    return {
      role,
      entries: list.map((entry) => entry.entry),
      steps,
      runs,
      passRate: judged > 0 ? passed / judged : undefined,
      roundsPerRun: runs > 0 ? steps / runs : 0,
      tokensPerRun: tokens > 0 && runs > 0 ? tokens / runs : undefined,
      usdPerRun:
        measured.length > 0 && priced && runs > 0 ? usd / runs : undefined,
      blockersPerReview:
        role === 'reviewer' && reviews > 0 ? blockers / reviews : undefined,
      medianSeconds: main?.duration?.medianSeconds ?? undefined,
    };
  });
}

/** One suite on the cost/quality plane, and whether it is on the frontier. */
export type ParetoPoint = {
  id: string;
  benchmark: string;
  label: string;
  score: number;
  tokensPerTrial: number;
  frontier: boolean;
  recommended: boolean;
};

export function paretoPoints(
  suites: SuiteInsight[],
  standings: Map<
    string,
    { pareto: boolean; recommended: boolean; tokensPerTrial: number }
  >
): ParetoPoint[] {
  return suites.flatMap((suite) => {
    const standing = standings.get(suite.id);
    if (
      !standing ||
      suite.meanScore === null ||
      suite.meanScore === undefined
    ) {
      return [];
    }
    const agents = Object.values(suite.agents);
    return [
      {
        id: suite.id,
        benchmark: suite.benchmark,
        label:
          agents.length > 0
            ? [...new Set(agents)].join(' + ')
            : suite.benchmark,
        score: suite.meanScore,
        tokensPerTrial: standing.tokensPerTrial,
        frontier: standing.pareto,
        recommended: standing.recommended,
      },
    ];
  });
}
