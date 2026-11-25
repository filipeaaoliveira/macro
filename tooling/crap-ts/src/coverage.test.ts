import { describe, expect, it } from 'bun:test';
import { crap, type FileCoverage, lineCoverage } from './coverage';

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

describe('lineCoverage', () => {
  it('counts the statements starting in the function lines that ran', () => {
    expect(lineCoverage(FILE, 1, 6)).toBe(66.67);
    expect(lineCoverage(FILE, 2, 3)).toBe(100);
    expect(lineCoverage(FILE, 8, 10)).toBe(0);
  });

  it('knows nothing of a file it has no entry for, or lines with no statement', () => {
    expect(lineCoverage(undefined, 1, 6)).toBeNull();
    expect(lineCoverage(FILE, 7, 8)).toBeNull();
  });
});

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
