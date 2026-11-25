/**
 * `bun tooling/crap-ts/crap-ts.ts [--base <commit>] [--package <dir>] [--threshold <score>] [--vitest-timeout <seconds>]`
 *
 * Scores the TypeScript functions the change since the base commit touched
 * in one package (default `apps/web`), and fails when one is new or more
 * complex than at the base and scores above the threshold (default 30).
 *
 * It keeps the changed source files in the package's `src/` (no tests,
 * declarations, configs or generated code), runs the package's tests near
 * them under coverage (`vitest run <filters>`, v8, Istanbul JSON), reads each function's complexity
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
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
} from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { parseArgs } from 'node:util';
import {
  escapeGlob,
  isGenerated,
  isScored,
  readListing,
  testFilters,
} from './src/changed';
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
/**
 * How long vitest may run, in seconds: under the 300 seconds the documented
 * declaration gives the whole check.
 */
const DEFAULT_VITEST_TIMEOUT = 240;
const SOURCE_EXTENSIONS = ['ts', 'tsx', 'mts', 'cts'];

const PASSED = 0;
const FAILED = 1;
const NOT_RUN = 2;

/** Why the check could not run, said on its error stream with exit code 2. */
class NotRun extends Error {}

type Options = {
  base: string;
  packageDir: string;
  threshold: number;
  vitestTimeout: number;
};

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
      'vitest-timeout': {
        type: 'string',
        default: String(DEFAULT_VITEST_TIMEOUT),
      },
    },
  });
  requireRuntime();
  const root = realpathSync(
    git(process.cwd(), ['rev-parse', '--show-toplevel']).trim()
  );
  const packageDir = values.package.replace(/\/+$/, '');
  const threshold = positiveNumber('--threshold', values.threshold);
  const vitestTimeout = positiveNumber(
    '--vitest-timeout',
    values['vitest-timeout']
  );

  requireTools(join(root, packageDir));

  const base = values.base ?? process.env[BASE_VARIABLE] ?? '';
  if (base === '' || !hasCommit(root, base)) {
    console.log(`${NAME}: skipped: no base commit to compare with`);
    return PASSED;
  }

  warnIfDirty(root);
  return score(root, { base, packageDir, threshold, vitestTimeout });
}

function positiveNumber(flag: string, text: string): number {
  const value = Number(text);
  if (text.trim() === '' || !Number.isFinite(value) || value <= 0) {
    throw new NotRun(
      `${flag} ${JSON.stringify(text)} is not a positive number`
    );
  }

  return value;
}

/**
 * It diffs the committed `HEAD` but reads and tests the working tree, so
 * uncommitted changes make the rows disagree with the diff.
 */
function warnIfDirty(root: string) {
  const status = git(root, ['status', '--porcelain', '--untracked-files=no']);
  if (status.trim() !== '') {
    console.error(
      `${NAME}: warning: the working tree has uncommitted changes; it scores the working tree against the diff of the committed HEAD`
    );
  }
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
  const { coverage, testsFailed } = await testCoverage(root, options, scored);
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

/** What a vitest run under coverage left. */
type TestRun = { coverage: Coverage; testsFailed: boolean };

/** The part of vitest's JSON report the check reads. */
type TestReport = { numFailedTests: number };

/**
 * Runs the package's tests near the touched files under coverage (see
 * `testFilters`). A failing test does not stop the check: the tests' own
 * check reports it, and the rows say their coverage is unproven. A run that
 * ended without one (a crash, an import error, an unhandled error, a
 * timeout) could not measure anything, and the check does not run.
 */
async function testCoverage(
  root: string,
  options: Options,
  touched: TouchedFile[]
): Promise<TestRun> {
  const packageRoot = join(root, options.packageDir);
  const files = touched.map((file) =>
    relative(packageRoot, join(root, file.diff.path))
  );
  const filters = testFilters(files, (folder) =>
    testFilesIn(join(packageRoot, folder))
  );
  if (filters.length === 0) {
    console.error(`${NAME}: no test sits near the touched files`);
    return { coverage: {}, testsFailed: false };
  }

  const reports = mkdtempSync(join(tmpdir(), `${NAME}-`));
  try {
    const testReport = join(reports, 'tests.json');
    const ended = await runVitest(
      packageRoot,
      [
        'run',
        '--passWithNoTests',
        ...filters,
        '--reporter=default',
        '--reporter=json',
        `--outputFile.json=${testReport}`,
        '--coverage.enabled',
        '--coverage.provider=v8',
        '--coverage.reporter=json',
        '--coverage.reportOnFailure',
        `--coverage.reportsDirectory=${reports}`,
        ...files.map((file) => `--coverage.include=${escapeGlob(file)}`),
      ],
      options.vitestTimeout
    );

    return readTestRun(ended, testReport, join(reports, 'coverage-final.json'));
  } finally {
    rmSync(reports, { recursive: true, force: true });
  }
}

/** The names in `folder`, none when it is not there. */
function testFilesIn(folder: string): string[] {
  try {
    return readdirSync(folder);
  } catch {
    return [];
  }
}

/** How vitest ended: its exit code, or the signal that stopped it. */
type Ended = { code: number | null; signal: string | null };

/**
 * Runs vitest in `packageRoot` in a process group of its own, stopping the
 * whole group (its workers too) after `timeout` seconds. Its output goes
 * to the error stream so the rows stay last.
 */
function runVitest(
  packageRoot: string,
  args: string[],
  timeout: number
): Promise<Ended> {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['x', 'vitest', ...args], {
      cwd: packageRoot,
      detached: true,
      stdio: ['ignore', 2, 2],
    });
    const timer = setTimeout(() => {
      stopGroup(child.pid);
      reject(new NotRun(`vitest did not finish in ${timeout} seconds`));
    }, timeout * 1000);

    child.on('error', (error) => {
      clearTimeout(timer);
      reject(new NotRun(`could not run vitest: ${error.message}`));
    });
    child.on('exit', (code, signal) => {
      clearTimeout(timer);
      // Workers it left behind would hold the machine after the check.
      stopGroup(child.pid);
      resolve({ code, signal });
    });
  });
}

function stopGroup(pid: number | undefined) {
  if (pid === undefined) {
    return;
  }

  try {
    process.kill(-pid, 'SIGKILL');
  } catch {
    // The group has already ended.
  }
}

function readTestRun(
  ended: Ended,
  testReport: string,
  coverageReport: string
): TestRun {
  const how =
    ended.signal === null
      ? `exited with ${ended.code}`
      : `was stopped by ${ended.signal}`;
  if (!existsSync(testReport) || !existsSync(coverageReport)) {
    throw new NotRun(`vitest ${how} and wrote no test or coverage report`);
  }

  const report: TestReport = JSON.parse(readFileSync(testReport, 'utf8'));
  const testsFailed = report.numFailedTests > 0;
  if (ended.code !== 0 && !testsFailed) {
    throw new NotRun(
      `vitest ${how} with no failed test: a crash, an import error or an unhandled error (see its output above)`
    );
  }

  return {
    coverage: JSON.parse(readFileSync(coverageReport, 'utf8')),
    testsFailed,
  };
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
