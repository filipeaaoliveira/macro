/**
 * Runs the check on throwaway repositories holding a package `pkg`.
 *
 * The tests that run vitest link the package's `node_modules` to
 * `CRAP_TS_TEST_NODE_MODULES`, else to this repository's, and need vitest,
 * `@vitest/coverage-v8`, `vite-plugin-solid`, `solid-js` and `jsdom` there.
 * This repository does not depend on the coverage provider, so they skip
 * unless it is installed, for instance in a throwaway folder:
 *
 *   bun add --exact vitest@<version> @vitest/coverage-v8@<version> vite-plugin-solid solid-js jsdom
 *   CRAP_TS_TEST_NODE_MODULES=<folder>/node_modules bun test tooling/crap-ts
 */
import { afterEach, describe, expect, it } from 'bun:test';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import {
  existsSync,
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
const NODE_MODULES =
  process.env.CRAP_TS_TEST_NODE_MODULES ??
  join(import.meta.dir, '..', '..', 'node_modules');
const HAS_PROVIDER = existsSync(
  join(NODE_MODULES, '@vitest', 'coverage-v8', 'package.json')
);
/** The Solid component test also needs these. */
const HAS_SOLID = ['vite-plugin-solid', 'solid-js', 'jsdom'].every((name) =>
  existsSync(join(NODE_MODULES, name, 'package.json'))
);
if (HAS_PROVIDER && !HAS_SOLID) {
  console.warn(
    `crap-ts: skipping the Solid component test: vite-plugin-solid, solid-js or jsdom is not in ${NODE_MODULES}`
  );
}
if (!HAS_PROVIDER) {
  console.warn(
    `crap-ts: skipping the tests that run vitest: @vitest/coverage-v8 is not in ${NODE_MODULES}; set CRAP_TS_TEST_NODE_MODULES to a node_modules that has it (see this file's header)`
  );
}

/** A real vitest run takes a few seconds. */
const TIMEOUT = 60_000;

/** Tools the check looks for before it runs, without vitest itself. */
const FAKE_TOOLS = {
  'pkg/node_modules/vitest/package.json': '{"name":"vitest","version":"5.1.0"}',
  'pkg/node_modules/@vitest/coverage-v8/package.json':
    '{"name":"@vitest/coverage-v8","version":"5.1.0"}',
};

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

/** A test of `grade` imported from `from`. */
function gradeTest(from: string) {
  return `import { expect, it } from 'vitest';
import { grade } from '${from}';

it('grades', () => {
  expect([95, 85, 75, 65, 55, 45, 5].map(grade)).toEqual([
    'a', 'b', 'c', 'd', 'e', 'f', 'g',
  ]);
});
`;
}

const repositories: string[] = [];

afterEach(() => {
  for (const repository of repositories.splice(0)) {
    rmSync(repository, { recursive: true, force: true });
  }
});

/**
 * A repository whose first commit holds `files` in a package `pkg`, its
 * `node_modules` linked to the test tools unless `files` brings its own.
 */
function repository(files: Record<string, string>, packageDir = 'pkg') {
  const root = mkdtempSync(join(tmpdir(), 'crap-ts-test-'));
  repositories.push(root);
  git(root, 'init', '-q', '-b', 'main');
  git(root, 'config', 'user.email', 'test@example.com');
  git(root, 'config', 'user.name', 'Test');
  write(root, {
    '.gitignore': 'node_modules\n',
    [join(packageDir, 'package.json')]: '{"name":"pkg","type":"module"}',
    ...files,
  });
  if (!existsSync(join(root, packageDir, 'node_modules'))) {
    symlinkSync(NODE_MODULES, join(root, packageDir, 'node_modules'));
  }

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

function check(root: string, args: string[], env: Record<string, string> = {}) {
  const result = spawnSync(
    process.execPath,
    [SCRIPT, '--package', 'pkg', ...args],
    {
      cwd: root,
      encoding: 'utf8',
      env: {
        ...process.env,
        MACRO_CHECK_BASE: '',
        MACRO_CHECK_CHANGED: '',
        ...env,
      },
    }
  );
  const lines = result.stdout.trim().split('\n');
  const rows = lines.slice(0, -1).map((line) => JSON.parse(line));

  return {
    status: result.status,
    rows,
    summary: lines.at(-1) ?? '',
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

describe.skipIf(!HAS_PROVIDER)('crap-ts with vitest', () => {
  it(
    'fails once the test covering a new complex function is removed',
    () => {
      const { root, base } = repository({
        'pkg/src/label.ts': LABEL,
        'pkg/src/label.test.ts': LABEL_TEST,
      });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': gradeTest('./grade'),
      });
      commit(root);

      const tested = check(root, ['--base', base]);
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

      const untested = check(root, ['--base', base]);
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
    'scores an uncalled arrow in an imported file as untested, like a function',
    () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/pick.ts': `${GRADE.replace(
          'export function grade(n: number) {',
          'export const pick = (n: number) => {'
        ).replace(/}\n$/, '};\n')}export class Picker {
  pick = (n: number) => {
    if (n > 90) return 'a';
    if (n > 80) return 'b';
    if (n > 70) return 'c';
    if (n > 60) return 'd';
    if (n > 50) return 'e';
    if (n > 40) return 'f';
    return 'g';
  };
}
export const one = () => 1;
`,
        'pkg/src/pick.test.ts': `import { expect, it } from 'vitest';
import { one } from './pick';

it('imports', () => {
  expect(one()).toBe(1);
});
`,
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.status).toBe(1);
      expect(
        result.rows.map((row) => [row.name, row.coverage, row.score, row.fails])
      ).toEqual([
        ['one', 100, 1, false],
        ['pick', 0, 56, true],
        ['Picker.pick', 0, 56, true],
      ]);
    },
    TIMEOUT
  );

  it.skipIf(!HAS_SOLID)(
    "does not count a Solid component's templates as running it",
    () => {
      const { root, base } = repository({
        'pkg/vitest.config.ts': `import solid from 'vite-plugin-solid';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [solid()],
  test: { environment: 'jsdom' },
});
`,
        'pkg/src/label.ts': LABEL,
      });
      write(root, {
        'pkg/src/view.tsx': `export function View(props: { n: number }) {
  return (
    <div>
      <span>{props.n > 90 ? 'a' : 'b'}</span>
      <span>{props.n > 80 ? 'a' : 'b'}</span>
      <span>{props.n > 70 ? 'a' : 'b'}</span>
      <span>{props.n > 60 ? 'a' : 'b'}</span>
      <span>{props.n > 50 ? 'a' : 'b'}</span>
      <p>{props.n > 40 ? 'a' : 'b'}</p>
    </div>
  );
}
export const one = () => 1;
`,
        'pkg/src/view.test.tsx': `import { expect, it } from 'vitest';
import { one } from './view';

it('imports', () => {
  expect(one()).toBe(1);
});
`,
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.status).toBe(1);
      expect(result.rows.at(-1)).toMatchObject({
        name: 'View',
        complexity: 7,
        coverage: 0,
        fails: true,
      });
    },
    TIMEOUT
  );

  it(
    'runs only the tests beside a shallow file',
    () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/more/grade.test.ts': gradeTest('../grade'),
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.status).toBe(1);
      expect(result.rows.at(-1)).toMatchObject({ name: 'grade', fails: true });
      expect(result.stderr).toContain('no test sits near the touched files');
    },
    TIMEOUT
  );

  it(
    "runs the tests of a deep file's feature",
    () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/features/runs/view/grade.ts': GRADE,
        'pkg/src/features/runs/grade.test.ts': gradeTest('./view/grade'),
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.status).toBe(0);
      expect(result.rows.at(-1)).toMatchObject({
        name: 'grade',
        coverage: 100,
      });
    },
    TIMEOUT
  );

  it(
    'covers a file whose path holds glob characters',
    () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/(group)/[id].ts': GRADE,
        'pkg/src/(group)/[id].test.ts': gradeTest('./[id]'),
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.status).toBe(0);
      expect(result.rows.at(-1)).toMatchObject({
        path: 'pkg/src/(group)/[id].ts',
        coverage: 100,
      });
    },
    TIMEOUT
  );

  it(
    'reports uncovered functions as unproven when a test fails',
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

      const result = check(root, ['--base', base]);
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

  it(
    'cannot run when a test file fails to import, with no failed test',
    () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': `import './grade';
throw new Error('broken on import');
`,
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.status).toBe(2);
      expect(result.stdout).toBe('');
      expect(result.stderr).toContain('with no failed test');
    },
    TIMEOUT
  );

  it(
    'cannot run when vitest outlasts its timeout',
    () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': `import { it } from 'vitest';
import './grade';

it('waits', async () => {
  await new Promise((resolve) => setTimeout(resolve, 20_000));
}, 30_000);
`,
      });
      commit(root);

      const result = check(root, ['--base', base, '--vitest-timeout', '3']);

      expect(result.status).toBe(2);
      expect(result.stderr).toContain('vitest did not finish in 3 seconds');
    },
    TIMEOUT
  );
  it(
    "ignores a package's own coverage thresholds",
    () => {
      const { root, base } = repository({
        'pkg/vitest.config.ts': `import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: { coverage: { thresholds: { 100: true } } },
});
`,
        'pkg/src/label.ts': LABEL,
      });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': `import { expect, it } from 'vitest';
import { grade } from './grade';

it('grades some', () => {
  expect([95, 85, 75, 65].map(grade)).toEqual(['a', 'b', 'c', 'd']);
});
`,
      });
      commit(root);

      const result = check(root, ['--base', base]);

      expect(result.stderr).not.toContain('could not');
      expect(result.status).toBe(0);
      expect(result.rows.at(-1)).toMatchObject({ name: 'grade', fails: false });
      expect(result.rows.at(-1)?.coverage).toBeLessThan(100);
    },
    TIMEOUT
  );

  it(
    'scores a package at the repository root',
    () => {
      const { root, base } = repository({ 'src/label.ts': LABEL }, '.');
      write(root, {
        'src/grade.ts': GRADE,
        'src/grade.test.ts': gradeTest('./grade'),
      });
      commit(root);

      const result = check(root, ['--base', base, '--package', '.']);

      expect(result.status).toBe(0);
      expect(result.rows).toEqual([
        expect.objectContaining({
          path: 'src/grade.ts',
          coverage: 100,
          fails: false,
        }),
      ]);
    },
    TIMEOUT
  );

  it(
    'stops vitest and its workers when the check is stopped',
    async () => {
      const { root, base } = repository({ 'pkg/src/label.ts': LABEL });
      write(root, {
        'pkg/src/grade.ts': GRADE,
        'pkg/src/grade.test.ts': `import { it } from 'vitest';
import './grade';

it('waits', async () => {
  console.error(\`worker \${process.pid} ready\`);
  await new Promise((resolve) => setTimeout(resolve, 30_000));
}, 40_000);
`,
      });
      commit(root);

      const child = spawn(
        process.execPath,
        [SCRIPT, '--package', 'pkg', '--base', base],
        { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] }
      );
      let stderr = '';
      const worker = await new Promise<number>((resolve) => {
        child.stderr.on('data', (chunk: Buffer) => {
          stderr += chunk.toString();
          const ready = /worker (\d+) ready/.exec(stderr);
          if (ready) {
            resolve(Number(ready[1]));
          }
        });
      });
      const ended = new Promise<number | null>((resolve) =>
        child.on('exit', (code) => resolve(code))
      );

      child.kill('SIGTERM');

      expect(await ended).toBe(2);
      expect(stderr).toContain('crap-ts: stopped by SIGTERM');
      expect(isRunning(worker)).toBe(false);
    },
    TIMEOUT
  );
});

/** Whether a process is still there, a moment after it was killed. */
function isRunning(pid: number): boolean {
  for (let attempt = 0; attempt < 20; attempt++) {
    try {
      process.kill(pid, 0);
    } catch {
      return false;
    }
    Bun.sleepSync(100);
  }

  return true;
}

describe('crap-ts before vitest', () => {
  it('keeps to the listed paths when the listing is complete', () => {
    const { root, base } = repository({
      ...FAKE_TOOLS,
      'pkg/src/label.ts': LABEL,
    });
    write(root, { 'pkg/src/grade.ts': GRADE });
    commit(root);
    const listing = join(root, '..', `${base}.changed`);
    writeFileSync(listing, 'complete\npkg/src/other.ts\n');
    repositories.push(listing);

    const result = check(root, [], {
      MACRO_CHECK_BASE: base,
      MACRO_CHECK_CHANGED: listing,
    });

    expect(result.status).toBe(0);
    expect(result.summary).toBe(
      'crap-ts: the change touched no scored TypeScript source'
    );
  });

  it('skips without a base, or with one the repository lacks', () => {
    const { root } = repository({ ...FAKE_TOOLS, 'pkg/src/label.ts': LABEL });

    for (const args of [
      [],
      ['--base', '0123456789abcdef0123456789abcdef01234567'],
    ]) {
      const result = check(root, args);
      expect(result.status).toBe(0);
      expect(result.summary).toBe(
        'crap-ts: skipped: no base commit to compare with'
      );
    }
  });

  it('warns when the working tree has uncommitted changes', () => {
    const { root, base } = repository({
      ...FAKE_TOOLS,
      'pkg/src/label.ts': LABEL,
    });
    write(root, { 'pkg/src/label.ts': `${LABEL}// changed\n` });

    const result = check(root, ['--base', base]);

    expect(result.status).toBe(0);
    expect(result.stderr).toContain('the working tree has uncommitted changes');
  });

  it('cannot run with an empty or invalid threshold', () => {
    const { root, base } = repository({
      ...FAKE_TOOLS,
      'pkg/src/label.ts': LABEL,
    });

    for (const threshold of ['', 'high', '-1']) {
      const result = check(root, ['--base', base, '--threshold', threshold]);
      expect(result.status).toBe(2);
      expect(result.stderr).toContain('--threshold');
    }
  });

  it('cannot run without the coverage provider, and says how to install it', () => {
    const { root, base } = repository({
      'pkg/src/label.ts': LABEL,
      'pkg/node_modules/vitest/package.json':
        '{"name":"vitest","version":"5.1.0"}',
    });

    const result = check(root, ['--base', base]);

    expect(result.status).toBe(2);
    expect(result.stderr).toBe(
      `crap-ts: @vitest/coverage-v8 5.1.0 is not installed in ${join(realpathSync(root), 'pkg')}: run bun add -d --exact @vitest/coverage-v8@5.1.0 there\n`
    );
  });
});
