import { describe, expect, it } from 'vitest';
import {
  calibrationReading,
  labelDisagrees,
  type RunScore,
  type ScorerCalibration,
  type SuiteInsight,
  suiteStandings,
  uncalibratedScorers,
} from './evaluation';

/**
 * A finished suite of `scored` trials, all scored, unless `changes` says
 * otherwise.
 */
const suite = (
  id: string,
  benchmark: string,
  meanScore: number | null,
  outputTokens: number,
  scored: number,
  changes: Partial<SuiteInsight> = {}
): SuiteInsight => ({
  id,
  benchmark,
  agents: {},
  createdAt: '2026-11-10T09:00:00Z',
  scored,
  infrastructure: 0,
  meanScore,
  tokens: { input: 0, output: outputTokens, cacheRead: 0, cacheWrite: 0 },
  trials: Array.from({ length: scored }, (_, index) => ({
    task: `task-${index}`,
    runId: `${id}-run-${index}`,
    startedAt: '2026-11-10T09:00:00Z',
    score: meanScore,
    scores: [],
  })),
  ...changes,
});

const marks = (standings: ReturnType<typeof suiteStandings>) =>
  Object.fromEntries(
    [...standings].map(([id, standing]) => [
      id,
      { pareto: standing.pareto, recommended: standing.recommended },
    ])
  );

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

    expect(marks(standings)).toEqual({
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

    expect(standings.get('late')).toEqual({
      pareto: true,
      recommended: true,
      tokensPerTrial: 250,
    });
    expect(standings.get('early')).toEqual({
      pareto: false,
      recommended: false,
      tokensPerTrial: 400,
    });
  });

  it('on the same score, leaves the dearer suite off the set and recommends the cheaper', () => {
    const standings = suiteStandings([
      suite('dear', 'core', 0.8, 900, 1),
      suite('cheap', 'core', 0.8, 300, 1),
    ]);

    expect(marks(standings)).toEqual({
      cheap: { pareto: true, recommended: true },
      dear: { pareto: false, recommended: false },
    });
  });

  it('reads scores that differ only past the shown precision as the same score', () => {
    // 0.1 + 0.2 is a hair above 0.3, but both read 0.30: the cheaper wins.
    const standings = suiteStandings([
      suite('hair', 'core', 0.1 + 0.2, 900, 1),
      suite('cheap', 'core', 0.3, 300, 1),
    ]);

    expect(marks(standings)).toEqual({
      cheap: { pareto: true, recommended: true },
      hair: { pareto: false, recommended: false },
    });
  });

  it('keeps both suites that tie on score and cost on the set, and recommends the newest', () => {
    const standings = suiteStandings([
      suite('older', 'core', 0.8, 300, 1, {
        createdAt: '2026-11-10T09:00:00Z',
      }),
      suite('newer', 'core', 0.8, 300, 1, {
        createdAt: '2026-11-12T09:00:00Z',
      }),
      suite('oldest', 'core', 0.8, 300, 1, {
        createdAt: '2026-11-08T09:00:00Z',
      }),
    ]);

    expect(marks(standings)).toEqual({
      older: { pareto: true, recommended: false },
      newer: { pareto: true, recommended: true },
      oldest: { pareto: true, recommended: false },
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

    expect(marks(standings)).toEqual({
      'core-a': { pareto: true, recommended: false },
      'core-b': { pareto: true, recommended: true },
      'docs-a': { pareto: true, recommended: true },
      'docs-b': { pareto: true, recommended: false },
    });
  });

  it('ranks a suite only once all its trials ended, lost ones included', () => {
    // running has one of ten trials scored, with a high score for few
    // tokens. lossy lost one of its three to infrastructure and is done.
    const running = suite('running', 'core', 1, 10, 1);
    const lossy = suite('lossy', 'core', 0.4, 200, 2);
    const standings = suiteStandings([
      suite('done', 'core', 0.6, 300, 1),
      {
        ...running,
        trials: [
          ...running.trials,
          ...Array.from({ length: 9 }, (_, index) => ({
            task: `waiting-${index}`,
            runId: `running-waiting-${index}`,
            startedAt: '2026-11-10T09:00:00Z',
            scores: [],
          })),
        ],
      },
      {
        ...lossy,
        infrastructure: 1,
        trials: [
          ...lossy.trials,
          {
            task: 'lost',
            runId: 'lossy-lost',
            startedAt: '2026-11-10T09:00:00Z',
            scores: [],
          },
        ],
      },
    ]);

    expect(marks(standings)).toEqual({
      done: { pareto: true, recommended: true },
      lossy: { pareto: true, recommended: false },
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

  it('compares no suite whose trials have no score, whatever its mean says', () => {
    // unjudged has a scored trial but no mean; empty a mean but no scored
    // trial, which would divide its tokens by none.
    const standings = suiteStandings([
      suite('scored', 'core', 0.5, 300, 1),
      suite('unjudged', 'core', null, 300, 1),
      suite('empty', 'core', 0.5, 0, 0),
    ]);

    expect(standings.size).toBe(0);
  });
});

const calibration = (
  changes: Partial<ScorerCalibration> = {}
): ScorerCalibration => ({
  labelled: 0,
  agreed: 0,
  agreement: null,
  needed: 20,
  threshold: 0.8,
  calibrated: false,
  ...changes,
});

describe('calibrationReading', () => {
  it('counts the labels an unlabelled scorer still needs', () => {
    expect(calibrationReading(calibration())).toEqual({
      kind: 'needsLabels',
      remaining: 20,
    });
    expect(
      calibrationReading(
        calibration({ labelled: 15, agreed: 15, agreement: 1 })
      )
    ).toEqual({ kind: 'needsLabels', remaining: 5 });
  });

  it('says a scorer with its labels agrees too little until the service calibrates it', () => {
    expect(
      calibrationReading(
        calibration({ labelled: 20, agreed: 14, agreement: 0.7 })
      )
    ).toEqual({ kind: 'disagrees', agreement: 0.7, threshold: 0.8 });
  });

  it('reads a calibrated scorer with its agreement', () => {
    expect(
      calibrationReading(
        calibration({
          labelled: 20,
          agreed: 18,
          agreement: 0.9,
          calibrated: true,
        })
      )
    ).toEqual({ kind: 'calibrated', agreement: 0.9 });
  });
});

describe('labelDisagrees', () => {
  const score = (passed: boolean, label?: boolean): RunScore => ({
    scorer: 'clear',
    score: passed ? 0.8 : 0.2,
    passed,
    reason: '',
    model: 'judge-small',
    judgedAt: '2026-11-16T10:00:00Z',
    byCurrentJudge: true,
    calibration: calibration(),
    label:
      label === undefined
        ? null
        : { passed: label, labelledAt: '2026-11-17T10:00:00Z' },
  });

  it('tells a label against the judge from one with it, and none from an unlabelled judgment', () => {
    expect(labelDisagrees(score(true, false))).toBe(true);
    expect(labelDisagrees(score(false, true))).toBe(true);
    expect(labelDisagrees(score(true, true))).toBe(false);
    expect(labelDisagrees(score(false))).toBeUndefined();
  });
});

describe('uncalibratedScorers', () => {
  it('names the scorers not calibrated yet', () => {
    expect(
      uncalibratedScorers([
        { name: 'clear', calibration: calibration() },
        { name: 'correct', calibration: calibration({ calibrated: true }) },
      ])
    ).toEqual(new Set(['clear']));
  });
});
