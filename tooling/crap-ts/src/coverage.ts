/**
 * Line coverage of a function from vitest's Istanbul JSON
 * (`coverage-final.json`), counted as crap4ts counts it: the share of the
 * statements starting inside the function's lines that the tests ran.
 * Reading the function's own lines also covers what crap4ts misses: a
 * function whose signature wraps onto several lines.
 */

type Position = { line: number; column: number | null };

/** One file's entry in `coverage-final.json`. */
export type FileCoverage = {
  path: string;
  statementMap: Record<string, { start: Position; end: Position }>;
  s: Record<string, number>;
};

/** `coverage-final.json`: each file's entry by its absolute path. */
export type Coverage = Record<string, FileCoverage>;

/**
 * The percent of the statements from `line` to `endLine` the tests ran;
 * null when the coverage has none there (no entry for the file, or no
 * statement in those lines).
 */
export function lineCoverage(
  file: FileCoverage | undefined,
  line: number,
  endLine: number
): number | null {
  if (!file) {
    return null;
  }

  let total = 0;
  let covered = 0;
  for (const [id, statement] of Object.entries(file.statementMap)) {
    const start = statement.start.line;
    if (start < line || start > endLine) {
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

/** CRAP = CC² × (1 − coverage)³ + CC; no coverage counts as none. */
export function crap(complexity: number, coverage: number | null): number {
  const uncovered = 1 - (coverage ?? 0) / 100;
  return round(complexity ** 2 * uncovered ** 3 + complexity);
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}
