/**
 * Runs the check on throwaway repositories: a package whose `node_modules`
 * links to this repository's, so vitest and its coverage provider run for
 * real.
 */
import { afterEach, describe, expect, it } from 'bun:test';
import { execFileSync, spawnSync } from 'node:child_process';
import {
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';

const SCRIPT = join(import.meta.dir, 'crap-ts.ts');
const NODE_MODULES = join(import.meta.dir, '..', '..', 'node_modules');
/** A real vitest run takes a few seconds. */
const TIMEOUT = 60_000;

const LABEL = `export function label(n: number) {
  return n > 0 ? 'up' : 'down';
}
`;

const LABEL_TEST = `import { expect, it } from 'vitest';
import { label } from './label';

it('labels', () => {
  expect(label(1)).toBe('up');
});
`;

/** CC 7: untested it scores 56, tested 7. */
const GRADE = `export function grade(n: number) {
  if (n > 90) return 'a';
  if (n > 80) return 'b';
  if (n > 70) return 'c';
  if (n > 60) return 'd';
  if (n > 50) return 'e';
  if (n > 40) return 'f';
  return 'g';
}
`;

const GRADE_TEST = `import { expect, it } from 'vitest';
import { grade } from './grade';

it('grades', () => {
  expect([95, 85, 75, 65, 55, 45, 5].map(grade)).toEqual([
    'a', 'b', 'c', 'd', 'e', 'f', 'g',
  ]);
});
`;

const repositories: string[] = [];

afterEach(() => {
  for (const repository of repositories.splice(0)) {
    rmSync(repository, { recursive: true, force: true });
  }
});

/** A repository whose first commit holds `files` and a package `pkg`; its commit. */
function repository(files: Record<string, string>, linked = true) {
  const root = mkdtempSync(join(tmpdir(), 'crap-ts-test-'));
  repositories.push(root);
  git(root, 'init', '-q', '-b', 'main');
  git(root, 'config', 'user.email', 'test@example.com');
  git(root, 'config', 'user.name', 'Test');
  write(root, {
    'pkg/package.json': '{"name":"pkg","type":"module"}',
    ...files,
  });
  if (linked) {
    symlinkSync(NODE_MODULES, join(root, 'pkg', 'node_modules'));
  }
  writeFileSync(join(root, '.gitignore'), 'node_modules\n');

  return { root, base: commit(root) };
}

function write(root: string, files: Record<string, string>) {
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
}

function commit(root: string): string {
  git(root, 'add', '-A');
  git(root, 'commit', '-q', '-m', 'change');
  return git(root, 'rev-parse', 'HEAD').trim();
}

function git(root: string, ...args: string[]): string {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8' });
}

function check(root: string, ...args: string[]) {
  const result = spawnSync(
    process.execPath,
    [SCRIPT, '--package', 'pkg', ...args],
    {
      cwd: root,
      encoding: 'utf8',
      env: { ...process.env, MACRO_CHECK_BASE: '', MACRO_CHECK_CHANGED: '' },
    }
  );
  const lines = result.stdout.trim().split('\n');
  const rows = lines.slice(0, -1).map((line) => JSON.parse(line));

  return {
    status: result.status,
    rows,
    summary: lines.at(-1) ?? '',
    stderr: result.stderr,
  };
}

describe('crap-ts', () => {
  it(
    'fails once the test covering a new complex function is removed',
    () => {
      const { root, base } = repository({
        'pkg/src/label.ts': LABEL,
        'pkg/src/label.test.ts': LABEL_TEST,
      });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': GRADE_TEST,
      });
      commit(root);

      const tested = check(root, '--base', base);
      expect(tested.status).toBe(0);
      expect(tested.rows).toEqual([
        {
          path: 'pkg/src/grade.ts',
          name: 'grade',
          line: 1,
          complexity: 7,
          coverage: 100,
          score: 7,
          change: 'new',
          base_complexity: null,
          fails: false,
        },
      ]);

      rmSync(join(root, 'pkg/src/grade.test.ts'));
      commit(root);

      const untested = check(root, '--base', base);
      expect(untested.status).toBe(1);
      expect(untested.rows.at(-1)).toMatchObject({
        name: 'grade',
        coverage: 0,
        score: 56,
        fails: true,
      });
      expect(untested.summary).toBe(
        'crap-ts: 1 touched functions; 1 new or more complex above 30 (failing); 0 others above it'
      );
    },
    TIMEOUT
  );

  it(
    'reports uncovered functions as unproven when a related test fails',
    () => {
      const { root, base } = repository({
        'pkg/src/label.ts': LABEL,
        'pkg/src/label.test.ts': LABEL_TEST,
      });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': `import { expect, it } from 'vitest';
import './grade';

it('breaks', () => {
  expect(1).toBe(2);
});
`,
      });
      commit(root);

      const result = check(root, '--base', base);
      expect(result.status).toBe(0);
      expect(result.rows).toEqual([
        expect.objectContaining({
          name: 'grade',
          fails: false,
          unproven: true,
        }),
      ]);
      expect(result.summary).toContain(
        'tests failed while coverage was collected, so 1 functions with no coverage are unproven'
      );
    },
    TIMEOUT
  );

  it('keeps to the listed paths when the listing is complete', () => {
    const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
    write(root, { 'pkg/src/grade.ts': GRADE });
    commit(root);
    const listing = join(root, '..', `${base}.changed`);
    writeFileSync(listing, 'complete\npkg/src/other.ts\n');
    repositories.push(listing);

    const result = spawnSync(process.execPath, [SCRIPT, '--package', 'pkg'], {
      cwd: root,
      encoding: 'utf8',
      env: {
        ...process.env,
        MACRO_CHECK_BASE: base,
        MACRO_CHECK_CHANGED: listing,
      },
    });

    expect(result.status).toBe(0);
    expect(result.stdout).toBe(
      'crap-ts: the change touched no scored TypeScript source\n'
    );
  });

  it('skips without a base, or with one the repository lacks', () => {
    const { root } = repository({ 'pkg/src/label.ts': LABEL });

    for (const args of [
      [],
      ['--base', '0123456789abcdef0123456789abcdef01234567'],
    ]) {
      const result = check(root, ...args);
      expect(result.status).toBe(0);
      expect(result.summary).toBe(
        'crap-ts: skipped: no base commit to compare with'
      );
    }
  });

  it('cannot run without the coverage provider, and says how to install it', () => {
    const { root, base } = repository(
      {
        'pkg/src/label.ts': LABEL,
        'pkg/node_modules/vitest/package.json':
          '{"name":"vitest","version":"5.1.0"}',
      },
      false
    );

    const result = check(root, '--base', base);

    expect(result.status).toBe(2);
    expect(result.stderr).toBe(
      `crap-ts: @vitest/coverage-v8 5.1.0 is not installed in ${join(realpathSync(root), 'pkg')}: run bun add -d --exact @vitest/coverage-v8@5.1.0 there\n`
    );
  });
});
