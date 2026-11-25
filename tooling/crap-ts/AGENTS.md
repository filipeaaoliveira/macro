# crap-ts

## What it is

Part of the factory's checks: `bun tooling/crap-ts/crap-ts.ts`, the
TypeScript CRAP check. It scores the functions a change touched in one
package (default `apps/web`) and fails when one is new or more complex than
at the base and scores above 30. The Rust side is `crap-rust`.

## How it works

1. Base: `--base`, else `MACRO_CHECK_BASE`; none, or one the repository
   lacks, prints `skipped: no base commit to compare with` and exits 0.
2. Changed paths: `MACRO_CHECK_CHANGED`'s list when its first line is
   `complete`, else every TypeScript path in the package
   `git diff <base> HEAD` names. `src/changed.ts` keeps source under the
   package's `src/` only.
3. Complexity of each function at the change (working tree) and at the
   base (`git show`), with the TypeScript compiler API (`src/functions.ts`).
4. Coverage: `vitest run <filters>` (`testFilters`): a file three or more
   folders deep runs the tests under that folder, a shallower one only the
   test files beside it. v8 provider, Istanbul JSON, statements inside each
   function's body (`src/coverage.ts`). Failed tests in vitest's JSON
   report make uncovered rows `unproven`; any other failed run, or one past
   `--vitest-timeout`, exits 2.
5. Rows (`src/rows.ts`): one JSON line per function whose lines meet a diff
   hunk, failing and worst last, then a summary line. Exit code: 0 passed or
   skipped, 1 a function failed, 2 not run.

## Start here

- `src/rows.ts`: pairing with the base, `change`, and when a row `fails`.
- `crap-ts.ts`: the commands it runs, in order.

## Checks

- `bun test tooling/crap-ts`. The end-to-end tests run vitest in throwaway
  repositories and skip, saying why, unless `@vitest/coverage-v8` is in
  `CRAP_TS_TEST_NODE_MODULES` (or this repository's `node_modules`, which
  does not depend on it); `crap-ts.test.ts`'s header says what to install.

## Gotchas

- The checks round keeps only the end of a check's output (4 KiB): vitest's
  output goes to stderr, the rows to stdout, failing and worst last.
- As in crap4ts, a nested function (callback, inner helper) is neither
  scored nor counted in the function holding it; the threshold was chosen
  on those counts (see `src/functions.ts`).
- It diffs the committed `HEAD` but reads the working tree: commit before
  running it by hand.
- Needs `@vitest/coverage-v8` at the package's vitest version, which this
  repository does not install; it says so and exits 2 when missing.

## Related

- Docs: `docs/development/factory-crap.md`
