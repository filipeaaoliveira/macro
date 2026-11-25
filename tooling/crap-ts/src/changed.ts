/**
 * Which changed files the check scores.
 *
 * A factory check is told the changed paths in the file `MACRO_CHECK_CHANGED`
 * names: its first line says whether the list is `complete`, and each line
 * after it is one path from the repository's root. Only a complete list
 * narrows the diff; otherwise the check diffs everything against the base.
 */

/** Every path the change touched, or null when the diff against the base decides. */
export function readListing(text: string): string[] | null {
  const [first, ...paths] = text.split('\n');
  if (first?.trim() !== 'complete') {
    return null;
  }

  return paths.filter((path) => path !== '');
}

const SOURCE = /\.(ts|tsx|mts|cts)$/;

/** Test files and declarations, which the check never scores. */
const SKIPPED_FILE =
  /(\.d\.[mc]?ts|\.(test|spec)\.[mc]?tsx?|\.config\.[mc]?ts)$/;

/** Folders that hold tests, generated or third-party code. */
const SKIPPED_FOLDERS = new Set([
  '__tests__',
  '__mocks__',
  'test',
  'tests',
  'browser-test',
  'generated',
  'node_modules',
  'dist',
]);

/**
 * Whether the check scores the file at `path`, from the repository's root:
 * TypeScript source in `packageDir`'s `src/`, no tests, declarations,
 * configs or generated code. Files outside `src/` (`vite.config.ts`,
 * `codegen.ts`) set the package up and have no tests of their own.
 */
export function isScored(path: string, packageDir: string): boolean {
  if (!path.startsWith(`${packageDir}/src/`)) {
    return false;
  }

  if (!SOURCE.test(path) || SKIPPED_FILE.test(path)) {
    return false;
  }

  const folders = path
    .slice(packageDir.length + 1)
    .split('/')
    .slice(0, -1);
  return !folders.some((folder) => SKIPPED_FOLDERS.has(folder));
}

/** How many lines at a file's top a generated-code marker is looked for in. */
const GENERATED_MARKER_LINES = 5;

/** Whether a file's text says it is generated (`@generated` near its top). */
export function isGenerated(source: string): boolean {
  return source
    .split('\n', GENERATED_MARKER_LINES)
    .some((line) => line.includes('@generated'));
}

/**
 * How many folders deep, from the package's root, the tests run for a
 * touched file are looked for: `src/features/runs/view/row.tsx` runs the
 * tests under `src/features/runs/`.
 */
const TEST_FOLDER_DEPTH = 3;

const TEST_FILE = /\.(test|spec)\.[mc]?[jt]sx?$/;

/**
 * The filters naming the tests that run for the touched files, as paths
 * from the package's root; empty when no test applies, never every test.
 *
 * A file at least `TEST_FOLDER_DEPTH` folders deep runs the tests under its
 * folder cut to that depth, which stands for the feature it belongs to. A
 * shallower file (`src/index.ts`) runs only the test files beside it, which
 * `testFilesIn` lists for a folder: its folder's subtree would be most of
 * the package. Vitest's own related-tests mode follows imports
 * transitively, and in an app whose tests import most of it, that runs most
 * of the suite for any change.
 */
export function testFilters(
  files: string[],
  testFilesIn: (folder: string) => string[]
): string[] {
  const folders = new Set<string>();
  const testFiles = new Set<string>();

  for (const file of files) {
    const parts = file.split('/').slice(0, -1);
    if (parts.length >= TEST_FOLDER_DEPTH) {
      folders.add(`${parts.slice(0, TEST_FOLDER_DEPTH).join('/')}/`);
      continue;
    }

    const folder = parts.join('/');
    for (const name of testFilesIn(folder)) {
      if (TEST_FILE.test(name)) {
        testFiles.add(folder === '' ? name : `${folder}/${name}`);
      }
    }
  }

  // A test inside a folder already listed adds nothing.
  const inFolders = (path: string) =>
    [...folders].some((folder) => path.startsWith(folder));
  return [
    ...folders,
    ...[...testFiles].filter((path) => !inFolders(path)),
  ].sort();
}

/**
 * `path` with the characters a glob reads escaped, so a coverage include
 * names that one file: `src/(group)/[id].tsx`.
 */
export function escapeGlob(path: string): string {
  return path.replace(/[\\*?[\]{}()!+@]/g, '\\$&');
}
