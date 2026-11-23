import { describe, expect, it } from 'vitest';
import { type SuiteInsight, suiteStandings } from './evaluation';

const suite = (
  id: string,
  benchmark: string,
  meanScore: number | null,
  outputTokens: number,
  scored: number
): SuiteInsight => ({
  id,
  benchmark,
  agents: {},
  createdAt: '2026-11-10T09:00:00Z',
  scored,
  infrastructure: 0,
  meanScore,
  tokens: { input: 0, output: outputTokens, cacheRead: 0, cacheWrite: 0 },
  trials: [],
});

describe('suiteStandings', () => {
  it('puts on the Pareto set the suites no other beats on score and cost, and recommends the best score on it', () => {
    // Per trial: haiku 0.6 for 100, sonnet 0.8 for 300, opus 0.9 for 900.
    // mixed scores 0.7 for 400: sonnet scores more for less, so it is off.
    const standings = suiteStandings([
      suite('haiku', 'core', 0.6, 200, 2),
      suite('sonnet', 'core', 0.8, 600, 2),
      suite('opus', 'core', 0.9, 1800, 2),
      suite('mixed', 'core', 0.7, 800, 2),
    ]);

    expect(Object.fromEntries(standings)).toEqual({
      haiku: { pareto: true, recommended: false },
      sonnet: { pareto: true, recommended: false },
      opus: { pareto: true, recommended: true },
      mixed: { pareto: false, recommended: false },
    });
  });

  it('compares tokens per scored trial, not the suite’s total', () => {
    // late used more tokens in all, over four trials to early's one: 250 a
    // trial against 400, at the same score, so late beats early.
    const standings = suiteStandings([
      suite('early', 'core', 0.5, 400, 1),
      suite('late', 'core', 0.5, 1000, 4),
    ]);

    expect(standings.get('late')).toEqual({ pareto: true, recommended: true });
    expect(standings.get('early')).toEqual({
      pareto: false,
      recommended: false,
    });
  });

  it('on the same score, leaves the dearer suite off the set and recommends the cheaper', () => {
    const standings = suiteStandings([
      suite('dear', 'core', 0.8, 900, 1),
      suite('cheap', 'core', 0.8, 300, 1),
    ]);

    expect(standings.get('cheap')).toEqual({ pareto: true, recommended: true });
    expect(standings.get('dear')).toEqual({
      pareto: false,
      recommended: false,
    });
  });

  it('keeps both suites that tie on score and cost on the set, and recommends the first', () => {
    const standings = suiteStandings([
      suite('first', 'core', 0.8, 300, 1),
      suite('second', 'core', 0.8, 300, 1),
    ]);

    expect(standings.get('first')).toEqual({ pareto: true, recommended: true });
    expect(standings.get('second')).toEqual({
      pareto: true,
      recommended: false,
    });
  });

  it('compares suites only within their benchmark', () => {
    // docs' suite would beat both core suites, but it replayed other tasks.
    const standings = suiteStandings([
      suite('core-a', 'core', 0.5, 300, 1),
      suite('core-b', 'core', 0.6, 400, 1),
      suite('docs-a', 'docs', 0.9, 100, 1),
      suite('docs-b', 'docs', 0.4, 50, 1),
    ]);

    expect(Object.fromEntries(standings)).toEqual({
      'core-a': { pareto: true, recommended: false },
      'core-b': { pareto: true, recommended: true },
      'docs-a': { pareto: true, recommended: true },
      'docs-b': { pareto: true, recommended: false },
    });
  });

  it('gives no standing to a suite with no score, nor to a benchmark with one suite to compare', () => {
    const standings = suiteStandings([
      suite('scored', 'core', 0.5, 300, 1),
      suite('waiting', 'core', null, 0, 0),
      suite('alone', 'docs', 0.9, 100, 1),
    ]);

    expect(standings.size).toBe(0);
  });
});
