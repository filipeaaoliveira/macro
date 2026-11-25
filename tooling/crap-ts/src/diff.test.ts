import { describe, expect, it } from 'bun:test';
import { lineAtBase, meets, parseDiff, touches } from './diff';

const DIFF = `diff --git apps/web/src/a.ts apps/web/src/a.ts
index 1..2 100644
--- apps/web/src/a.ts
+++ apps/web/src/a.ts
@@ -5,0 +6,2 @@ function a() {
+  one();
+  two();
@@ -20,2 +22 @@ function b() {
-  three();
-  four();
+  five();
diff --git apps/web/src/gone.ts apps/web/src/gone.ts
deleted file mode 100644
--- apps/web/src/gone.ts
+++ /dev/null
@@ -1,3 +0,0 @@
-export function gone() {
-  return 1;
-}
diff --git apps/web/src/new name.ts apps/web/src/new name.ts
new file mode 100644
--- /dev/null
+++ apps/web/src/new name.ts\t
@@ -0,0 +1 @@
+export const x = 1;
`;

describe('parseDiff', () => {
  it('reads each file with its base path and both sides of its hunks', () => {
    expect(parseDiff(DIFF)).toEqual([
      {
        path: 'apps/web/src/a.ts',
        basePath: 'apps/web/src/a.ts',
        hunks: [
          { baseStart: 5, baseCount: 0, start: 6, count: 2 },
          { baseStart: 20, baseCount: 2, start: 22, count: 1 },
        ],
      },
      {
        path: null,
        basePath: 'apps/web/src/gone.ts',
        hunks: [{ baseStart: 1, baseCount: 3, start: 0, count: 0 }],
      },
      {
        path: 'apps/web/src/new name.ts',
        basePath: null,
        hunks: [{ baseStart: 0, baseCount: 0, start: 1, count: 1 }],
      },
    ]);
  });
});

describe('meets', () => {
  it('meets a function whose lines it overlaps', () => {
    const hunk = { baseStart: 10, baseCount: 1, start: 10, count: 3 };

    expect(meets(hunk, 12, 20)).toBe(true);
    expect(meets(hunk, 1, 10)).toBe(true);
    expect(meets(hunk, 13, 20)).toBe(false);
    expect(meets(hunk, 1, 9)).toBe(false);
  });

  it('meets a function only when a removal falls between two of its lines', () => {
    const removal = { baseStart: 8, baseCount: 2, start: 7, count: 0 };

    expect(meets(removal, 5, 9)).toBe(true);
    expect(meets(removal, 7, 9)).toBe(true);
    expect(meets(removal, 1, 7)).toBe(false);
    expect(meets(removal, 8, 12)).toBe(false);
  });
});

describe('touches', () => {
  it('touches a function any hunk meets', () => {
    const [file] = parseDiff(DIFF);

    expect(touches(file, 1, 6)).toBe(true);
    expect(touches(file, 9, 21)).toBe(false);
    expect(touches(file, 22, 30)).toBe(true);
  });
});

describe('lineAtBase', () => {
  it('shifts lines by what the hunks before them added or removed', () => {
    const [file] = parseDiff(DIFF);

    expect(lineAtBase(file, 3)).toBe(3);
    expect(lineAtBase(file, 8)).toBe(6);
    expect(lineAtBase(file, 21)).toBe(19);
    expect(lineAtBase(file, 30)).toBe(29);
  });

  it('maps a line inside a hunk to the hunk at the base', () => {
    const [file] = parseDiff(DIFF);

    expect(lineAtBase(file, 6)).toBe(6);
    expect(lineAtBase(file, 22)).toBe(20);
  });
});
