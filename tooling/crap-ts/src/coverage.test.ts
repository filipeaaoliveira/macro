import { describe, expect, it } from 'bun:test';
import { bodyCoverage, crap, type FileCoverage } from './coverage';

const FILE: FileCoverage = {
  path: '/repo/apps/web/src/a.ts',
  statementMap: {
    '0': { start: { line: 2, column: 0 }, end: { line: 2, column: 9 } },
    '1': { start: { line: 3, column: 0 }, end: { line: 3, column: 9 } },
    '2': { start: { line: 4, column: 0 }, end: { line: 6, column: 1 } },
    '3': { start: { line: 9, column: 0 }, end: { line: 9, column: 9 } },
  },
  s: { '0': 1, '1': 4, '2': 0, '3': 0 },
};

describe('bodyCoverage', () => {
  it('counts the statements starting in the body that ran', () => {
    expect(bodyCoverage(FILE, at(1, 0), at(6, 1))).toBe(66.67);
    expect(bodyCoverage(FILE, at(2, 0), at(3, 9))).toBe(100);
    expect(bodyCoverage(FILE, at(8, 0), at(10, 0))).toBe(0);
  });

  it('leaves out a statement starting before the body on its first line', () => {
    // `export const pick = (n) => {` on line 2: the declaration's own
    // statement (run on import) starts at column 0, the body at column 20.
    expect(bodyCoverage(FILE, at(2, 20), at(6, 1))).toBe(50);
  });

  it('knows nothing of a file it has no entry for, or a body with no statement', () => {
    expect(bodyCoverage(undefined, at(1, 0), at(6, 1))).toBeNull();
    expect(bodyCoverage(FILE, at(7, 0), at(8, 9))).toBeNull();
  });
});

function at(line: number, column: number) {
  return { line, column };
}

describe('crap', () => {
  it('scores complexity against the share left untested', () => {
    expect(crap(12, 0)).toBe(156);
    expect(crap(12, 100)).toBe(12);
    expect(crap(6, 50)).toBe(10.5);
  });

  it('scores unknown coverage as none', () => {
    expect(crap(5, null)).toBe(30);
  });
});
