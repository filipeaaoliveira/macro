/**
 * Coverage of a function from vitest's Istanbul JSON
 * (`coverage-final.json`), counted as crap4ts counts line coverage: the
 * share of the statements in the function that the tests ran. Only the
 * statements inside its body count, so a variable or class property holding
 * an arrow, which runs on import, does not make an uncalled arrow look run,
 * and a signature wrapped onto several lines (which crap4ts misses) does not
 * matter.
 */
import type { Position } from './functions';

type StatementPosition = { line: number; column: number | null };

/** One file's entry in `coverage-final.json`. */
export type FileCoverage = {
  path: string;
  statementMap: Record<
    string,
    { start: StatementPosition; end: StatementPosition }
  >;
  s: Record<string, number>;
};

/** `coverage-final.json`: each file's entry by its absolute path. */
export type Coverage = Record<string, FileCoverage>;

/**
 * The percent of the statements starting from `from` to `to` the tests
 * ran; null when the coverage has none there (no entry for the file, or no
 * statement in the body).
 */
export function bodyCoverage(
  file: FileCoverage | undefined,
  from: Position,
  to: Position
): number | null {
  if (!file) {
    return null;
  }

  let total = 0;
  let covered = 0;
  for (const [id, statement] of Object.entries(file.statementMap)) {
    const start = {
      line: statement.start.line,
      column: statement.start.column ?? 0,
    };
    if (before(start, from) || before(to, start)) {
      continue;
    }

    total += 1;
    if ((file.s[id] ?? 0) > 0) {
      covered += 1;
    }
  }

  if (total === 0) {
    return null;
  }

  return round((covered / total) * 100);
}

function before(a: Position, b: Position): boolean {
  return a.line < b.line || (a.line === b.line && a.column < b.column);
}

/** CRAP = CC² × (1 − coverage)³ + CC; no coverage counts as none. */
export function crap(complexity: number, coverage: number | null): number {
  const uncovered = 1 - (coverage ?? 0) / 100;
  return round(complexity ** 2 * uncovered ** 3 + complexity);
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}
