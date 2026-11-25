import { describe, expect, it } from 'bun:test';
import type { FileCoverage } from './coverage';
import type { FileDiff } from './diff';
import type { ScoredFunction } from './functions';
import { rows, type TouchedFile } from './rows';

const PATH = 'apps/web/src/a.ts';

function fn(
  name: string,
  line: number,
  endLine: number,
  complexity: number,
  body = `{ ${name} }`
): ScoredFunction {
  return { name, line, endLine, complexity, body };
}

/** Coverage with one statement per line from `first` to `last`, all run or none. */
function covered(first: number, last: number, ran: boolean): FileCoverage {
  const coverage: FileCoverage = { path: PATH, statementMap: {}, s: {} };
  for (let line = first; line <= last; line++) {
    coverage.statementMap[line] = {
      start: { line, column: 0 },
      end: { line, column: 1 },
    };
    coverage.s[line] = ran ? 1 : 0;
  }

  return coverage;
}

/** `apps/web/src/a.ts`, its lines 5 and 30 changed in place. */
const CHANGED_IN_PLACE: FileDiff & { path: string } = {
  path: PATH,
  basePath: PATH,
  hunks: [
    { baseStart: 5, baseCount: 1, start: 5, count: 1 },
    { baseStart: 30, baseCount: 1, start: 30, count: 1 },
  ],
};

function touched(
  functions: ScoredFunction[],
  coverage?: FileCoverage,
  diff = CHANGED_IN_PLACE
): TouchedFile {
  return { diff, functions, coverage };
}

describe('rows', () => {
  it('gives only touched functions a row', () => {
    const result = rows({
      touched: [touched([fn('touched', 1, 10, 2), fn('untouched', 12, 20, 9)])],
      base: new Map([[PATH, [fn('touched', 1, 10, 2)]]]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result.map((row) => row.name)).toEqual(['touched']);
  });

  it('fails a new or more complex function above the threshold', () => {
    const result = rows({
      touched: [touched([fn('added', 1, 10, 6), fn('grown', 22, 40, 7)])],
      base: new Map([[PATH, [fn('grown', 22, 40, 6)]]]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result).toEqual([
      {
        path: PATH,
        name: 'added',
        line: 1,
        complexity: 6,
        coverage: null,
        score: 42,
        change: 'new',
        base_complexity: null,
        fails: true,
      },
      {
        path: PATH,
        name: 'grown',
        line: 22,
        complexity: 7,
        coverage: null,
        score: 56,
        change: 'rose',
        base_complexity: 6,
        fails: true,
      },
    ]);
  });

  it('lists a function no more complex than at the base without failing it', () => {
    const result = rows({
      touched: [touched([fn('same', 1, 10, 6), fn('simpler', 22, 40, 6)])],
      base: new Map([[PATH, [fn('same', 1, 10, 6), fn('simpler', 22, 40, 8)]]]),
      threshold: 30,
      testsFailed: false,
    });

    expect(
      result.map((row) => [row.name, row.change, row.score, row.fails])
    ).toEqual([
      ['same', 'same', 42, false],
      ['simpler', 'fell', 42, false],
    ]);
  });

  it('passes a new function its tests cover', () => {
    const result = rows({
      touched: [touched([fn('added', 1, 10, 6)], covered(1, 10, true))],
      base: new Map(),
      threshold: 30,
      testsFailed: false,
    });

    expect(result[0]).toMatchObject({ coverage: 100, score: 6, fails: false });
  });

  it('puts failing rows last and the worst last', () => {
    const result = rows({
      touched: [
        touched([
          fn('worst', 1, 10, 9),
          fn('kept', 22, 40, 9),
          fn('failing', 28, 31, 7),
        ]),
      ],
      base: new Map([[PATH, [fn('kept', 22, 40, 9)]]]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result.map((row) => [row.name, row.fails])).toEqual([
      ['kept', false],
      ['failing', true],
      ['worst', true],
    ]);
  });

  it('pairs a function with the base function of its name nearest the line it came from', () => {
    // Ten lines were added at the top; the second `handler` grew.
    const diff: FileDiff & { path: string } = {
      path: PATH,
      basePath: PATH,
      hunks: [
        { baseStart: 0, baseCount: 0, start: 1, count: 10 },
        { baseStart: 25, baseCount: 1, start: 35, count: 1 },
      ],
    };
    const result = rows({
      touched: [
        touched(
          [fn('handler', 11, 20, 2), fn('handler', 31, 40, 8)],
          undefined,
          diff
        ),
      ],
      base: new Map([
        [PATH, [fn('handler', 1, 10, 2), fn('handler', 21, 30, 6)]],
      ]),
      threshold: 30,
      testsFailed: false,
    });

    expect(
      result.map((row) => [row.line, row.change, row.base_complexity])
    ).toEqual([[31, 'rose', 6]]);
  });

  it('pairs with the nearer base function of its name when an earlier one was removed', () => {
    // The first `handler` (lines 1-10) was removed; the second grew.
    const diff: FileDiff & { path: string } = {
      path: PATH,
      basePath: PATH,
      hunks: [
        { baseStart: 1, baseCount: 10, start: 0, count: 0 },
        { baseStart: 25, baseCount: 1, start: 15, count: 1 },
      ],
    };
    const result = rows({
      touched: [touched([fn('handler', 11, 20, 8)], undefined, diff)],
      base: new Map([
        [PATH, [fn('handler', 1, 10, 2), fn('handler', 21, 30, 6)]],
      ]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result.map((row) => [row.change, row.base_complexity])).toEqual([
      ['rose', 6],
    ]);
  });

  it('pairs a function in a renamed file under its earlier path', () => {
    const diff = { ...CHANGED_IN_PLACE, basePath: 'apps/web/src/old.ts' };
    const result = rows({
      touched: [touched([fn('moved', 1, 10, 9)], undefined, diff)],
      base: new Map([['apps/web/src/old.ts', [fn('moved', 1, 10, 9)]]]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result[0]).toMatchObject({ change: 'same', fails: false });
  });

  it('counts a renamed or moved function whose body matches a removed one as the same', () => {
    const result = rows({
      touched: [touched([fn('renamed', 1, 10, 9, '{ body }')])],
      base: new Map([
        [PATH, [fn('original', 1, 10, 9, '{ body }')]],
        ['apps/web/src/gone.ts', [fn('other', 1, 10, 9, '{ other }')]],
      ]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result[0]).toMatchObject({
      change: 'same',
      base_complexity: 9,
      fails: false,
    });
  });

  it('does not take a body match from a base function still paired by name', () => {
    const result = rows({
      touched: [
        touched([
          fn('copy', 1, 10, 9, '{ body }'),
          fn('kept', 22, 40, 9, '{ body }'),
        ]),
      ],
      base: new Map([[PATH, [fn('kept', 22, 40, 9, '{ body }')]]]),
      threshold: 30,
      testsFailed: false,
    });

    expect(result.find((row) => row.name === 'copy')).toMatchObject({
      change: 'new',
      fails: true,
    });
  });

  it('marks uncovered rows unproven, not failing, when tests failed', () => {
    const result = rows({
      touched: [
        touched(
          [fn('uncovered', 1, 10, 9), fn('covered', 22, 40, 9)],
          mergeCoverage(covered(1, 10, false), covered(22, 40, true))
        ),
      ],
      base: new Map(),
      threshold: 30,
      testsFailed: true,
    });

    expect(result.map((row) => [row.name, row.fails, row.unproven])).toEqual([
      ['covered', false, undefined],
      ['uncovered', false, true],
    ]);
  });
});

function mergeCoverage(a: FileCoverage, b: FileCoverage): FileCoverage {
  return {
    path: a.path,
    statementMap: { ...a.statementMap, ...b.statementMap },
    s: { ...a.s, ...b.s },
  };
}
