/**
 * The functions of a TypeScript file and their cyclomatic complexity, read
 * with the TypeScript compiler API.
 *
 * A file's functions are those crap4ts scores: its top-level function
 * declarations, its top-level variables holding a function or arrow, its
 * default export, and the methods, accessors, constructors and function-valued
 * properties of its top-level classes (`Class.method`). Complexity counts as
 * crap4ts counts it: 1, plus 1 for each `if`, `?:`, loop, `catch`, `case`
 * with statements, `&&`, `||`, `??`, and optional chain (once per chain).
 * As in crap4ts, a function nested in one of those (a callback, a helper
 * declared inside it) is neither scored nor counted in the function holding
 * it. Counting it there raised the factory-runs feature's functions above 30
 * from 7 to 28 of 399, mostly components with inline handlers, so the
 * threshold chosen on crap4ts's counts would no longer fit.
 */
import ts from 'typescript';

export type ScoredFunction = {
  /** Its name; `Class.member` for a class member. */
  name: string;
  /** The line its declaration starts on. */
  line: number;
  /** Its last line. */
  endLine: number;
  /** Its cyclomatic complexity. */
  complexity: number;
  /** Its parameters and body with whitespace collapsed, to find it again under another name. */
  body: string;
};

/** The functions in `source`, a file at `path` (its extension picks TSX or TS). */
export function functionsOf(source: string, path: string): ScoredFunction[] {
  const kind = path.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const file = ts.createSourceFile(
    path,
    source,
    ts.ScriptTarget.Latest,
    true,
    kind
  );
  const found: ScoredFunction[] = [];

  for (const statement of file.statements) {
    found.push(...statementFunctions(statement, file));
  }

  return found.sort((a, b) => a.line - b.line);
}

function statementFunctions(
  statement: ts.Statement,
  file: ts.SourceFile
): ScoredFunction[] {
  if (ts.isFunctionDeclaration(statement) && statement.body) {
    const name = statement.name?.text ?? 'default';
    return [scored(name, statement, statement, file)];
  }

  if (ts.isVariableStatement(statement)) {
    return statement.declarationList.declarations.flatMap((declaration) => {
      const value = declaration.initializer;
      if (!ts.isIdentifier(declaration.name) || !value || !isFunction(value)) {
        return [];
      }

      return [scored(declaration.name.text, statement, value, file)];
    });
  }

  if (ts.isExportAssignment(statement) && isFunction(statement.expression)) {
    return [scored('default', statement, statement.expression, file)];
  }

  if (ts.isClassDeclaration(statement) && statement.name) {
    const className = statement.name.text;
    return statement.members.flatMap((member) =>
      memberFunctions(className, member, file)
    );
  }

  return [];
}

function memberFunctions(
  className: string,
  member: ts.ClassElement,
  file: ts.SourceFile
): ScoredFunction[] {
  const isMethod =
    ts.isMethodDeclaration(member) ||
    ts.isGetAccessorDeclaration(member) ||
    ts.isSetAccessorDeclaration(member);

  if (isMethod && member.body) {
    const name = `${className}.${memberName(member.name, file)}`;
    return [scored(name, member, member, file)];
  }

  if (ts.isConstructorDeclaration(member) && member.body) {
    return [scored(`${className}.constructor`, member, member, file)];
  }

  if (
    ts.isPropertyDeclaration(member) &&
    member.initializer &&
    isFunction(member.initializer)
  ) {
    const name = `${className}.${memberName(member.name, file)}`;
    return [scored(name, member, member.initializer, file)];
  }

  return [];
}

function memberName(name: ts.PropertyName, file: ts.SourceFile): string {
  if (ts.isIdentifier(name) || ts.isPrivateIdentifier(name)) {
    return name.text;
  }

  if (ts.isStringLiteral(name) || ts.isNumericLiteral(name)) {
    return name.text;
  }

  return name.getText(file);
}

type FunctionNode =
  | ts.FunctionDeclaration
  | ts.FunctionExpression
  | ts.ArrowFunction
  | ts.MethodDeclaration
  | ts.GetAccessorDeclaration
  | ts.SetAccessorDeclaration
  | ts.ConstructorDeclaration;

function isFunction(
  node: ts.Node
): node is ts.FunctionExpression | ts.ArrowFunction {
  return ts.isFunctionExpression(node) || ts.isArrowFunction(node);
}

/** `declaration` gives the span, `fn` the parameters and body. */
function scored(
  name: string,
  declaration: ts.Node,
  fn: FunctionNode,
  file: ts.SourceFile
): ScoredFunction {
  const line = lineOf(file, declaration.getStart(file));
  const endLine = lineOf(file, declaration.getEnd());
  const parameters = fn.parameters.map((parameter) => parameter.getText(file));
  const body = fn.body ? fn.body.getText(file) : '';

  return {
    name,
    line,
    endLine,
    complexity: fn.body ? 1 + decisions(fn.body) : 1,
    body: `(${parameters.join(',')})${body}`.replace(/\s+/g, ' '),
  };
}

function lineOf(file: ts.SourceFile, position: number): number {
  return file.getLineAndCharacterOfPosition(position).line + 1;
}

const LOGICAL_OPERATORS = new Set([
  ts.SyntaxKind.AmpersandAmpersandToken,
  ts.SyntaxKind.BarBarToken,
  ts.SyntaxKind.QuestionQuestionToken,
]);

/** The decisions in `node` and under it, leaving out nested functions. */
function decisions(node: ts.Node): number {
  let count = isDecision(node) ? 1 : 0;
  node.forEachChild((child) => {
    if (!ts.isFunctionLike(child)) {
      count += decisions(child);
    }
  });

  return count;
}

function isDecision(node: ts.Node): boolean {
  if (ts.isCaseClause(node)) {
    return node.statements.length > 0;
  }

  if (ts.isBinaryExpression(node)) {
    return LOGICAL_OPERATORS.has(node.operatorToken.kind);
  }

  return BRANCHES.has(node.kind) || isChainEnd(node);
}

const BRANCHES = new Set([
  ts.SyntaxKind.IfStatement,
  ts.SyntaxKind.ConditionalExpression,
  ts.SyntaxKind.ForStatement,
  ts.SyntaxKind.ForInStatement,
  ts.SyntaxKind.ForOfStatement,
  ts.SyntaxKind.WhileStatement,
  ts.SyntaxKind.DoStatement,
  ts.SyntaxKind.CatchClause,
]);

/**
 * Whether `node` ends an optional chain: `a?.b.c` is one chain, counted once,
 * at `.c`.
 */
function isChainEnd(node: ts.Node): boolean {
  if (!ts.isOptionalChain(node)) {
    return false;
  }

  const parent = node.parent;
  return !(ts.isOptionalChain(parent) && parent.expression === node);
}
