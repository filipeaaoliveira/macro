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
 * TypeScript source inside `packageDir`, no tests, declarations, configs or
 * generated code.
 */
export function isScored(path: string, packageDir: string): boolean {
  if (!path.startsWith(`${packageDir}/`)) {
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
 * tests under `src/features/runs`.
 */
const TEST_FOLDER_DEPTH = 3;

/**
 * The folders whose tests run for the touched files, from the package's
 * root. Each file's folder, cut to `TEST_FOLDER_DEPTH`, stands for the
 * feature it belongs to. Vitest's own related-tests mode follows imports
 * transitively, and in an app whose tests import most of it, that runs most
 * of the suite for any change. Empty, every test runs: a touched file sits
 * at the package's root.
 */
export function testFolders(files: string[]): string[] {
  const folders = new Set(
    files.map((file) =>
      file.split('/').slice(0, -1).slice(0, TEST_FOLDER_DEPTH).join('/')
    )
  );
  if (folders.has('')) {
    return [];
  }

  // A folder inside another one listed adds no test.
  return [...folders]
    .filter(
      (folder) => ![...folders].some((other) => folder.startsWith(`${other}/`))
    )
    .sort();
}
