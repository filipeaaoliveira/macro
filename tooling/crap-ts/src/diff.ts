/**
 * The lines a change made, from `git diff --unified=0 --no-prefix`.
 */

/**
 * One hunk: `baseCount` lines from `baseStart` at the base became `count`
 * lines from `start`. A count of 0 is a side with no lines: the hunk then
 * sits just after that side's line `start`.
 */
export type Hunk = {
  baseStart: number;
  baseCount: number;
  start: number;
  count: number;
};

/** One file the diff names, and what it changed in it. */
export type FileDiff = {
  /** Its path from the repository's root; null when the change removed it. */
  path: string | null;
  /** Its path at the base: the earlier name of a renamed file; null when new. */
  basePath: string | null;
  hunks: Hunk[];
};

/** What `---` or `+++` names for a file that is not there. */
const NO_FILE = '/dev/null';

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** Reads a `git diff --unified=0 --no-prefix` output, removed files included. */
export function parseDiff(text: string): FileDiff[] {
  const files: FileDiff[] = [];
  let basePath: string | null = null;

  for (const line of text.split('\n')) {
    if (line.startsWith('--- ')) {
      basePath = fileName(line.slice(4));
      continue;
    }

    if (line.startsWith('+++ ')) {
      files.push({ path: fileName(line.slice(4)), basePath, hunks: [] });
      basePath = null;
      continue;
    }

    const header = HUNK_HEADER.exec(line);
    const file = files.at(-1);
    if (header && file) {
      file.hunks.push({
        baseStart: Number(header[1]),
        baseCount: header[2] === undefined ? 1 : Number(header[2]),
        start: Number(header[3]),
        count: header[4] === undefined ? 1 : Number(header[4]),
      });
    }
  }

  return files;
}

/** The path a `---` or `+++` line names. Git ends a name holding a space with a tab. */
function fileName(name: string): string | null {
  const trimmed = name.replace(/\t$/, '');
  return trimmed === NO_FILE ? null : trimmed;
}

/**
 * Whether the hunk changed a line of the function spanning `first` to
 * `last`, both included. A removal changes it when lines are removed between
 * two of its own.
 */
export function meets(hunk: Hunk, first: number, last: number): boolean {
  if (hunk.count === 0) {
    return first <= hunk.start && hunk.start < last;
  }

  const end = hunk.start + hunk.count - 1;
  return hunk.start <= last && first <= end;
}

/** Whether the change touched the function spanning `first` to `last`. */
export function touches(diff: FileDiff, first: number, last: number): boolean {
  return diff.hunks.some((hunk) => meets(hunk, first, last));
}

/**
 * The base line that the changed file's `line` came from: lines between
 * hunks shift by what the hunks before them added or removed. A line inside
 * a hunk maps to that hunk's first base line.
 */
export function lineAtBase(diff: FileDiff, line: number): number {
  let shift = 0;

  for (const hunk of diff.hunks) {
    const nextLine = afterHunk(hunk.start, hunk.count);
    if (line < nextLine && hunk.count > 0 && line >= hunk.start) {
      return hunk.baseCount === 0 ? hunk.baseStart + 1 : hunk.baseStart;
    }

    if (line < nextLine) {
      break;
    }

    shift = afterHunk(hunk.baseStart, hunk.baseCount) - nextLine;
  }

  return line + shift;
}

/** The first line after one side of a hunk. */
function afterHunk(start: number, count: number): number {
  return count === 0 ? start + 1 : start + count;
}
