/**
 * `bun tooling/crap-ts/crap-ts.ts [--base <commit>] [--package <dir>] [--threshold <score>]`
 *
 * Scores the TypeScript functions the change since the base commit touched
 * in one package (default `apps/web`), and fails when one is new or more
 * complex than at the base and scores above the threshold (default 30).
 *
 * It keeps the changed source files (no tests, declarations, configs or
 * generated code), runs the package's tests in their folders under coverage
 * (`vitest run <folders>`, v8, Istanbul JSON), reads each function's complexity
 * with the TypeScript compiler API at the change and at the base, and prints
 * one JSON line per touched function, those that fail last, then a summary
 * line. Exit code: 0 passed or skipped, 1 a function failed, 2 the check
 * could not run.
 *
 * The base is `--base`, else `MACRO_CHECK_BASE`; without one, or with one
 * the repository lacks, it skips. The changed paths are the ones
 * `MACRO_CHECK_CHANGED`'s file lists when its first line is `complete`, else
 * every path `git diff <base> HEAD` names.
 */
import { execFileSync, spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
} from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { parseArgs } from 'node:util';
import { isGenerated, isScored, readListing, testFolders } from './src/changed';
import type { Coverage } from './src/coverage';
import { type FileDiff, parseDiff, touches } from './src/diff';
import type { ScoredFunction } from './src/functions';
import { type Row, rows, type TouchedFile } from './src/rows';

const NAME = 'crap-ts';
/** The variable naming the commit the change is made against. */
const BASE_VARIABLE = 'MACRO_CHECK_BASE';
/** The variable naming the file that lists the changed paths. */
const CHANGED_VARIABLE = 'MACRO_CHECK_CHANGED';
/** crap4ts's lenient level and cargo-crap's default; see docs/development/factory-crap.md. */
const DEFAULT_THRESHOLD = 30;
const DEFAULT_PACKAGE = 'apps/web';
const SOURCE_EXTENSIONS = ['ts', 'tsx', 'mts', 'cts'];

const PASSED = 0;
const FAILED = 1;
const NOT_RUN = 2;

/** Why the check could not run, said on its error stream with exit code 2. */
class NotRun extends Error {}

type Options = { base: string; packageDir: string; threshold: number };

async function main(): Promise<number> {
  try {
    return await check();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`${NAME}: ${message}`);
    return NOT_RUN;
  }
}

async function check(): Promise<number> {
  const { values } = parseArgs({
    options: {
      base: { type: 'string' },
      package: { type: 'string', default: DEFAULT_PACKAGE },
      threshold: { type: 'string', default: String(DEFAULT_THRESHOLD) },
    },
  });
  requireRuntime();
  const root = realpathSync(
    git(process.cwd(), ['rev-parse', '--show-toplevel']).trim()
  );
  const packageDir = values.package.replace(/\/+$/, '');
  const threshold = Number(values.threshold);
  if (!Number.isFinite(threshold)) {
    throw new NotRun(`--threshold ${values.threshold} is not a number`);
  }

  requireTools(join(root, packageDir));

  const base = values.base ?? process.env[BASE_VARIABLE] ?? '';
  if (base === '' || !hasCommit(root, base)) {
    console.log(`${NAME}: skipped: no base commit to compare with`);
    return PASSED;
  }

  return score(root, { base, packageDir, threshold });
}

/** Fails fast when the check runs outside bun or without git. */
function requireRuntime() {
  if (!process.versions.bun) {
    throw new NotRun('run it with bun: bun tooling/crap-ts/crap-ts.ts');
  }

  const result = spawnSync('git', ['--version'], { stdio: 'ignore' });
  if (result.error || result.status !== 0) {
    throw new NotRun('git is not installed or not on PATH: install git');
  }
}

/** Fails fast, with how to install it, when a tool the check runs is missing. */
function requireTools(packageRoot: string) {
  const fromPackage = createRequire(join(packageRoot, 'package.json'));
  const vitest = versionOf(fromPackage, 'vitest');
  if (!vitest) {
    throw new NotRun(
      `vitest is not installed in ${packageRoot}: run bun install`
    );
  }

  const provider = versionOf(fromPackage, '@vitest/coverage-v8');
  if (provider !== vitest) {
    throw new NotRun(
      `@vitest/coverage-v8 ${vitest} is not installed in ${packageRoot}: run bun add -d --exact @vitest/coverage-v8@${vitest} there`
    );
  }

  if (!versionOf(createRequire(import.meta.url), 'typescript')) {
    throw new NotRun('typescript is not installed: run bun install');
  }
}

function versionOf(require: NodeJS.Require, name: string): string | undefined {
  try {
    const manifest = require.resolve(`${name}/package.json`);
    return JSON.parse(readFileSync(manifest, 'utf8')).version;
  } catch {
    return undefined;
  }
}

function hasCommit(root: string, base: string): boolean {
  const result = spawnSync('git', ['cat-file', '-e', `${base}^{commit}`], {
    cwd: root,
    stdio: 'ignore',
  });
  return result.status === 0;
}

async function score(root: string, options: Options): Promise<number> {
  const diffs = changedFiles(root, options);
  // Loaded once the tools are checked: it imports typescript.
  const { functionsOf } = await import('./src/functions');
  const touched = touchedFiles(root, diffs, functionsOf);
  if (touched.length === 0) {
    console.log(`${NAME}: the change touched no scored TypeScript source`);
    return PASSED;
  }

  const base = new Map<string, ScoredFunction[]>();
  for (const diff of diffs) {
    if (diff.basePath) {
      const source = git(root, ['show', `${options.base}:${diff.basePath}`]);
      base.set(diff.basePath, functionsOf(source, diff.basePath));
    }
  }

  // Only files with a touched function need coverage: a file of constants
  // imported everywhere would otherwise run most of the package's tests.
  const scored = touched.filter((file) =>
    file.functions.some((fn) => touches(file.diff, fn.line, fn.endLine))
  );
  const { coverage, testsFailed } =
    scored.length === 0
      ? { coverage: {}, testsFailed: false }
      : folderCoverage(root, options, scored);
  for (const file of scored) {
    file.coverage = coverage[join(root, file.diff.path)];
  }

  const result = rows({
    touched,
    base,
    threshold: options.threshold,
    testsFailed,
  });
  return print(result, options.threshold, testsFailed);
}

/** The scored files the diff against the base names, removed ones included. */
function changedFiles(root: string, options: Options): FileDiff[] {
  const listing = readChangedListing();
  const pathspec = listing
    ? listing.filter((path) => isScored(path, options.packageDir))
    : SOURCE_EXTENSIONS.map(
        (extension) => `:(glob)${options.packageDir}/**/*.${extension}`
      );
  if (pathspec.length === 0) {
    return [];
  }

  const text = git(root, [
    'diff',
    '--unified=0',
    '--no-prefix',
    '--find-renames',
    '--no-color',
    '--no-ext-diff',
    options.base,
    'HEAD',
    '--',
    ...pathspec,
  ]);
  return parseDiff(text).filter((diff) => {
    const path = diff.path ?? diff.basePath;
    return path !== null && isScored(path, options.packageDir);
  });
}

function readChangedListing(): string[] | null {
  const file = process.env[CHANGED_VARIABLE];
  if (!file || !existsSync(file)) {
    return null;
  }

  return readListing(readFileSync(file, 'utf8'));
}

/** The files the change left in the tree, as the working tree holds them. */
function touchedFiles(
  root: string,
  diffs: FileDiff[],
  functionsOf: (source: string, path: string) => ScoredFunction[]
): TouchedFile[] {
  const touched: TouchedFile[] = [];

  for (const diff of diffs) {
    const path = diff.path;
    if (path === null || diff.hunks.length === 0) {
      continue;
    }

    const source = readFileSync(join(root, path), 'utf8');
    if (!isGenerated(source)) {
      touched.push({
        diff: { ...diff, path },
        functions: functionsOf(source, path),
        coverage: undefined,
      });
    }
  }

  return touched;
}

/**
 * Runs the package's tests in the touched files' folders under coverage
 * (see `testFolders`). A failing test does not stop the check: the tests'
 * own check reports it, and the rows say their coverage is unproven.
 */
function folderCoverage(
  root: string,
  options: Options,
  touched: TouchedFile[]
): { coverage: Coverage; testsFailed: boolean } {
  const packageRoot = join(root, options.packageDir);
  const files = touched.map((file) =>
    relative(packageRoot, join(root, file.diff.path))
  );
  const reports = mkdtempSync(join(tmpdir(), `${NAME}-`));

  try {
    const result = spawnSync(
      process.execPath,
      [
        'x',
        'vitest',
        'run',
        '--passWithNoTests',
        ...testFolders(files),
        '--coverage.enabled',
        '--coverage.provider=v8',
        '--coverage.reporter=json',
        '--coverage.reportOnFailure',
        `--coverage.reportsDirectory=${reports}`,
        ...files.map((file) => `--coverage.include=${file}`),
      ],
      // Its output goes to the error stream so the rows stay last.
      { cwd: packageRoot, stdio: ['ignore', 2, 2] }
    );
    if (result.error) {
      throw new NotRun(`could not run vitest: ${result.error.message}`);
    }

    const report = join(reports, 'coverage-final.json');
    if (!existsSync(report)) {
      throw new NotRun(
        `vitest exited with ${result.status} and wrote no coverage`
      );
    }

    return {
      coverage: JSON.parse(readFileSync(report, 'utf8')),
      testsFailed: result.status !== 0,
    };
  } finally {
    rmSync(reports, { recursive: true, force: true });
  }
}

/** Prints the rows and a summary; the exit code. */
function print(result: Row[], threshold: number, testsFailed: boolean): number {
  for (const row of result) {
    console.log(JSON.stringify(row));
  }

  const failing = result.filter((row) => row.fails).length;
  const others = result.filter(
    (row) => !row.fails && row.score > threshold
  ).length;
  const unproven = result.filter((row) => row.unproven).length;
  const testsNote = testsFailed
    ? `; tests failed while coverage was collected, so ${unproven} functions with no coverage are unproven, not failing`
    : '';
  console.log(
    `${NAME}: ${result.length} touched functions; ${failing} new or more complex above ${threshold} (failing); ${others} others above it${testsNote}`
  );
  return failing > 0 ? FAILED : PASSED;
}

function git(cwd: string, args: string[]): string {
  try {
    return execFileSync('git', ['-c', 'core.quotePath=false', ...args], {
      cwd,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (error) {
    const stderr =
      error && typeof error === 'object' && 'stderr' in error
        ? String(error.stderr).trim()
        : String(error);
    throw new NotRun(`git ${args.join(' ')} failed: ${stderr}`);
  }
}

process.exit(await main());
