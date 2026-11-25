/**
 * The rows the check prints: one per touched function, its score at the
 * change joined with its complexity at the base.
 */
import { crap, type FileCoverage, lineCoverage } from './coverage';
import { type FileDiff, lineAtBase, touches } from './diff';
import type { ScoredFunction } from './functions';

/** How a touched function's complexity compares with the base's. */
export type Change = 'new' | 'rose' | 'same' | 'fell';

/** One touched function, as the check prints it. */
export type Row = {
  /** Its file, from the repository's root. */
  path: string;
  name: string;
  /** The line its declaration starts on. */
  line: number;
  complexity: number;
  /** The percent of its statements the related tests ran; null when unknown. */
  coverage: number | null;
  score: number;
  change: Change;
  /** Its complexity at the base; null for a new function. */
  base_complexity: number | null;
  /** New or more complex, and above the threshold. */
  fails: boolean;
  /**
   * Set when tests failed while coverage was collected and the function
   * shows no coverage: its score is not evidence, so it does not fail.
   */
  unproven?: true;
};

/** A file the change left in the tree, with its functions now. */
export type TouchedFile = {
  diff: FileDiff & { path: string };
  functions: ScoredFunction[];
  coverage: FileCoverage | undefined;
};

export type RowsInput = {
  touched: TouchedFile[];
  /** The functions of each base path the diff names, removed files included. */
  base: Map<string, ScoredFunction[]>;
  threshold: number;
  /** Whether a related test failed while coverage was collected. */
  testsFailed: boolean;
};

/**
 * The rows of the functions the change touched, those that fail last and
 * the worst last among them, so the end of the output, which the checks
 * round keeps, holds what matters.
 *
 * A function pairs with the base's function of its name in its file's base
 * path; among several of one name, with the one nearest the base line its
 * own line came from. A function left unpaired whose parameters and body
 * match a base function left unpaired (in any file the diff names) was
 * moved or renamed, and counts as that one.
 */
export function rows(input: RowsInput): Row[] {
  const pairs = new Map<ScoredFunction, ScoredFunction>();
  const pairedAtBase = new Set<ScoredFunction>();

  for (const file of input.touched) {
    const atBase = file.diff.basePath
      ? (input.base.get(file.diff.basePath) ?? [])
      : [];
    pairByName(file, atBase, pairs, pairedAtBase);
  }

  const removed = [...input.base.values()]
    .flat()
    .filter((fn) => !pairedAtBase.has(fn));
  const result: Row[] = [];

  for (const file of input.touched) {
    for (const fn of file.functions) {
      if (!touches(file.diff, fn.line, fn.endLine)) {
        continue;
      }

      const atBase = pairs.get(fn) ?? takeMoved(fn, removed);
      result.push(row(file, fn, atBase, input));
    }
  }

  return result.sort(
    (a, b) => Number(a.fails) - Number(b.fails) || a.score - b.score
  );
}

/** Pairs every function of the file, touched or not, with its base self. */
function pairByName(
  file: TouchedFile,
  atBase: ScoredFunction[],
  pairs: Map<ScoredFunction, ScoredFunction>,
  pairedAtBase: Set<ScoredFunction>
) {
  for (const fn of file.functions) {
    const baseLine = lineAtBase(file.diff, fn.line);
    let nearest: ScoredFunction | undefined;

    for (const candidate of atBase) {
      if (candidate.name !== fn.name || pairedAtBase.has(candidate)) {
        continue;
      }

      const distance = Math.abs(candidate.line - baseLine);
      if (!nearest || distance < Math.abs(nearest.line - baseLine)) {
        nearest = candidate;
      }
    }

    if (nearest) {
      pairs.set(fn, nearest);
      pairedAtBase.add(nearest);
    }
  }
}

/** The removed base function with the same parameters and body, taken from `removed`. */
function takeMoved(
  fn: ScoredFunction,
  removed: ScoredFunction[]
): ScoredFunction | undefined {
  const index = removed.findIndex((candidate) => candidate.body === fn.body);
  if (index === -1) {
    return undefined;
  }

  return removed.splice(index, 1)[0];
}

function row(
  file: TouchedFile,
  fn: ScoredFunction,
  atBase: ScoredFunction | undefined,
  input: RowsInput
): Row {
  const coverage = lineCoverage(file.coverage, fn.line, fn.endLine);
  const score = crap(fn.complexity, coverage);
  const change = changeOf(fn.complexity, atBase?.complexity);
  const riskier = change === 'new' || change === 'rose';
  const unproven = input.testsFailed && !coverage;

  const result: Row = {
    path: file.diff.path,
    name: fn.name,
    line: fn.line,
    complexity: fn.complexity,
    coverage,
    score,
    change,
    base_complexity: atBase?.complexity ?? null,
    fails: riskier && score > input.threshold && !unproven,
  };
  if (unproven) {
    result.unproven = true;
  }

  return result;
}

function changeOf(complexity: number, atBase: number | undefined): Change {
  if (atBase === undefined) {
    return 'new';
  }

  if (complexity > atBase) {
    return 'rose';
  }

  return complexity < atBase ? 'fell' : 'same';
}
