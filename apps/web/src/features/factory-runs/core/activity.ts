import type {
  FoldedMessage,
  MessagePart,
} from '@service-agent-fold/generated/types';

/**
 * One thing a Step's agent did lately, as its live tail shows it: a file it
 * edited or read, a command it ran and how that went, a search, or the
 * latest line it wrote.
 */
export type ActivityLine = {
  key: string;
  kind: 'edit' | 'read' | 'run' | 'search' | 'say';
  /** The file, the command, or the words. */
  text: string;
  /** A command's exit code, once it ended. */
  exitCode?: number;
  /** Whether it runs now. */
  live: boolean;
  failed: boolean;
};

/** The last path segment pair of `path`: `src/strings.ts`. */
function shortPath(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts.slice(-2).join('/');
}

/** The first line of `text`, trimmed of markdown marks. */
function firstLine(text: string): string {
  const line =
    text
      .split('\n')
      .map((part) => part.trim())
      .find((part) => part.length > 0) ?? '';
  return line.replace(/^[#>*\-\s]+/, '').replace(/[*`_]/g, '');
}

function lineOf(part: MessagePart, key: string): ActivityLine | undefined {
  if (part.kind === 'text') {
    const text = firstLine(part.text);
    return text
      ? { key, kind: 'say', text, live: false, failed: false }
      : undefined;
  }
  if (part.kind !== 'tool_use') return undefined;
  const live = part.status === 'running' || part.status === 'pending';
  const failed = part.status === 'failed';
  const detail = part.detail;
  switch (detail.kind) {
    case 'edit': {
      const path = detail.diffs[0]?.path;
      return path
        ? { key, kind: 'edit', text: shortPath(path), live, failed }
        : undefined;
    }
    case 'read': {
      const path = detail.paths[0];
      return path
        ? { key, kind: 'read', text: shortPath(path), live, failed }
        : undefined;
    }
    case 'search': {
      const path = detail.paths[0];
      return path
        ? { key, kind: 'search', text: shortPath(path), live, failed }
        : undefined;
    }
    case 'terminal':
      return detail.command
        ? {
            key,
            kind: 'run',
            text: detail.command.split('\n')[0] ?? detail.command,
            exitCode: detail.exitCode ?? undefined,
            live,
            failed: failed || (detail.exitCode ?? 0) !== 0,
          }
        : undefined;
    default:
      return undefined;
  }
}

/**
 * The last `limit` things the agent of a Step's session did, newest last:
 * its tool calls and the lines it wrote, from the folded transcript.
 */
export function sessionActivity(
  messages: readonly FoldedMessage[],
  limit = 4
): ActivityLine[] {
  const lines: ActivityLine[] = [];
  for (let m = messages.length - 1; m >= 0 && lines.length < limit; m--) {
    const message = messages[m];
    if (!message || message.author.kind === 'user') continue;
    const parts = message.parts;
    for (let p = parts.length - 1; p >= 0 && lines.length < limit; p--) {
      const part = parts[p];
      const line = part ? lineOf(part, `${message.key}:${p}`) : undefined;
      if (line) lines.push(line);
    }
  }
  return lines.reverse();
}
