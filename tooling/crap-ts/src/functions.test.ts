import { describe, expect, it } from 'bun:test';
import { functionsOf } from './functions';

describe('functionsOf', () => {
  it('finds top-level functions, variables, default exports and class members', () => {
    const source = `export function declared(a: number) {
  return a;
}
export const arrow = () => 1;
const expression = function () {};
const notAFunction = 3;
export class Box {
  constructor(private value: number) {}
  get size() {
    return this.value;
  }
  open() {}
  close = () => {};
}
export default () => {};
`;

    expect(
      functionsOf(source, 'a.ts').map((fn) => [fn.name, fn.line, fn.endLine])
    ).toEqual([
      ['declared', 1, 3],
      ['arrow', 4, 4],
      ['expression', 5, 5],
      ['Box.constructor', 8, 8],
      ['Box.size', 9, 11],
      ['Box.open', 12, 12],
      ['Box.close', 13, 13],
      ['default', 15, 15],
    ]);
  });

  it('counts each decision once', () => {
    const source = `function decide(a?: { b?: { c: number } }, list: number[] = []) {
  if (a) {
    for (const x of list) {
      while (x > 0 && list.length) {}
    }
  }
  for (let i = 0; i < 1; i++) {}
  for (const k in a) {}
  do {} while (false);
  try {} catch {}
  const v = a?.b?.c ?? (a ? 1 : 2);
  const w = a || list;
  switch (v) {
    case 1:
    case 2:
      return 1;
    case 3:
      return 2;
    default:
      return 3;
  }
}
`;

    // 1 + if, for-of, while, &&, for, for-in, do, catch, one chain, ??,
    // ?:, ||, and the two cases with statements.
    expect(functionsOf(source, 'a.ts')[0].complexity).toBe(15);
  });

  it('leaves out the decisions of the functions it holds', () => {
    const source = `export const List = (props: { items: string[] }) => {
  const label = (item: string) => (item ? item : 'none');
  return props.items.map((item) => (item.length > 1 ? label(item) : null));
};
`;

    expect(functionsOf(source, 'a.tsx')[0].complexity).toBe(1);
  });

  it('reads TSX', () => {
    const source = `export function View(props: { on: boolean }) {
  return <div>{props.on && <span />}</div>;
}
`;

    expect(functionsOf(source, 'a.tsx')[0].complexity).toBe(2);
  });

  it('gives a function the same body under another name or place', () => {
    const before = functionsOf(
      'function a(x: number) {\n  return x;\n}\n',
      'a.ts'
    );
    const after = functionsOf(
      '\n\nexport function b(x: number) { return x; }\n',
      'b.ts'
    );

    expect(after[0].body).toBe(before[0].body);
  });

  it('starts an arrow body after the variable or property holding it', () => {
    const source = `export const pick = (n: number) => {
  return n;
};
class Box {
  close = () =>
    1;
}
`;

    expect(
      functionsOf(source, 'a.ts').map((fn) => [fn.bodyStart, fn.bodyEnd])
    ).toEqual([
      [
        { line: 1, column: 35 },
        { line: 3, column: 1 },
      ],
      [
        { line: 6, column: 4 },
        { line: 6, column: 5 },
      ],
    ]);
  });
});
