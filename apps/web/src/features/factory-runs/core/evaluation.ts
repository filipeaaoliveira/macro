import type { SuiteInsight } from '@service-factory/generated/schemas';
import { tokenTotal } from './insights';

export type {
  AgentRuleInsight,
  ArmInsight,
  BenchmarkSummary,
  EvaluationInsights,
  ExperimentInsight,
  ImprovementInsight,
  ImprovementState,
  NamerStanding,
  ScorerInsight,
  SelfImprovementInsight,
  SuiteInsight,
} from '@service-factory/generated/schemas';

/** Where a suite stands among the other suites of its Benchmark. */
export type SuiteStanding = {
  /** No other suite of its Benchmark beats it on both score and cost. */
  pareto: boolean;
  /** The best score on the Pareto set. */
  recommended: boolean;
  /** Its cost: the tokens of its scored trials, per scored trial. */
  tokensPerTrial: number;
};

/**
 * Scores closer than this are the same score: they read the same at the
 * two decimals Insights shows, and a mean's rounding (0.1 + 0.2 against
 * 0.3) must not pick a side.
 */
const SCORE_TOLERANCE = 0.005;

type SuitePoint = {
  id: string;
  score: number;
  cost: number;
  createdAt: number;
};

/** How `a`'s score compares to `b`'s: 1 higher, -1 lower, 0 the same. */
function compareScores(a: SuitePoint, b: SuitePoint): number {
  const difference = a.score - b.score;
  if (Math.abs(difference) < SCORE_TOLERANCE) return 0;
  return Math.sign(difference);
}

/**
 * Whether `a` beats `b`: it scores at least as well and costs no more, and
 * is strictly better on one of the two.
 */
function dominates(a: SuitePoint, b: SuitePoint): boolean {
  const score = compareScores(a, b);
  const noWorse = score >= 0 && a.cost <= b.cost;
  const better = score > 0 || a.cost < b.cost;
  return noWorse && better;
}

/** Whether all of a suite's trials ended: scored, or lost to infrastructure. */
function finished(suite: SuiteInsight): boolean {
  return suite.scored + suite.infrastructure === suite.trials.length;
}

/**
 * Each suite's standing among the suites that replayed the same Benchmark,
 * by suite id, on two axes: its mean score (higher is better) and its
 * tokens per scored trial (lower is better). Tokens per trial rather than
 * the suite's total, so a suite that lost trials to infrastructure doesn't
 * look cheaper for it. Scores within `SCORE_TOLERANCE` are the same score.
 *
 * Only finished suites with a score are compared: a suite still running
 * has a score of the trials that ended so far, and one with no scored
 * trial has none. A Benchmark with fewer than two such suites gets no
 * standings, since a Pareto set of one says nothing.
 *
 * The recommended suite has the best score on the set. A suite with the
 * same score and fewer tokens beats the dearer one, so the set holds no
 * dearer tie; suites that tie on both go to the newest.
 */
export function suiteStandings(
  suites: SuiteInsight[]
): Map<string, SuiteStanding> {
  const byBenchmark = new Map<string, SuitePoint[]>();
  for (const suite of suites) {
    if (!finished(suite) || suite.scored === 0) continue;
    if (suite.meanScore === null || suite.meanScore === undefined) continue;

    const point = {
      id: suite.id,
      score: suite.meanScore,
      cost: tokenTotal(suite.tokens) / suite.scored,
      createdAt: Date.parse(suite.createdAt),
    };
    const points = byBenchmark.get(suite.benchmark) ?? [];
    points.push(point);
    byBenchmark.set(suite.benchmark, points);
  }

  const standings = new Map<string, SuiteStanding>();
  for (const points of byBenchmark.values()) {
    if (points.length < 2) continue;

    const front = points.filter(
      (point) => !points.some((other) => dominates(other, point))
    );

    const recommended = front.reduce((best, point) => {
      const score = compareScores(point, best);
      if (score !== 0) return score > 0 ? point : best;
      return point.createdAt > best.createdAt ? point : best;
    });

    for (const point of points) {
      standings.set(point.id, {
        pareto: front.includes(point),
        recommended: point === recommended,
        tokensPerTrial: point.cost,
      });
    }
  }
  return standings;
}
