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
  /** The best score on the Pareto set, ties going to the cheaper suite. */
  recommended: boolean;
};

type SuitePoint = { id: string; score: number; cost: number };

/**
 * Whether `a` beats `b`: it scores at least as well and costs no more, and
 * is strictly better on one of the two.
 */
function dominates(a: SuitePoint, b: SuitePoint): boolean {
  const noWorse = a.score >= b.score && a.cost <= b.cost;
  const better = a.score > b.score || a.cost < b.cost;
  return noWorse && better;
}

/**
 * Each suite's standing among the suites that replayed the same Benchmark,
 * by suite id, on two axes: its mean score (higher is better) and its
 * tokens per scored trial (lower is better). Tokens per trial rather than
 * the suite's total, so a suite that lost trials to infrastructure doesn't
 * look cheaper for it.
 *
 * A suite with no scored trial has no score to compare and gets no
 * standing. Neither does a Benchmark with fewer than two suites to
 * compare, where a Pareto set would say nothing. Two suites with the same
 * score and cost are both on the set; the first of them in `suites` is
 * recommended.
 */
export function suiteStandings(
  suites: SuiteInsight[]
): Map<string, SuiteStanding> {
  const byBenchmark = new Map<string, SuitePoint[]>();
  for (const suite of suites) {
    if (suite.scored === 0) continue;
    if (suite.meanScore === null || suite.meanScore === undefined) continue;

    const point = {
      id: suite.id,
      score: suite.meanScore,
      cost: tokenTotal(suite.tokens) / suite.scored,
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

    // The best score on the set. A suite with the same score and a lower
    // cost beats the dearer one, so the set holds no dearer tie; suites
    // that tie on both go to the earlier, since `reduce` keeps the one it
    // holds.
    const recommended = front.reduce((best, point) =>
      point.score > best.score ? point : best
    );

    for (const point of points) {
      standings.set(point.id, {
        pareto: front.includes(point),
        recommended: point === recommended,
      });
    }
  }
  return standings;
}
