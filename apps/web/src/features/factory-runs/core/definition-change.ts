import type { ProposeDefinitionChangeRequest } from '@service-factory/generated/schemas';

/** The most bytes a change's title holds, as the factory keeps it. */
export const MAX_CHANGE_TITLE_BYTES = 256;

/** The most bytes a change's description holds. */
export const MAX_CHANGE_DESCRIPTION_BYTES = 16 * 1024;

/** The Definition's files a change writes, apart from its Flows'. */
const DEFINITION_FILES = [
  'factory.json',
  'agents.json',
  'rules.json',
  'scorers.json',
] as const;

/** A Flow's file: `flows/{name}.json`, one level down. */
const FLOW_FILE = /^flows\/[^/\s]+\.json$/;

/** A file of a change being written: its path and its whole new text. */
export type DraftFile = { path: string; text: string };

/** A change to the Definition, as the person writes it. */
export type DefinitionChangeDraft = {
  title: string;
  description: string;
  files: DraftFile[];
};

/**
 * What stops a file going: a path the Definition doesn't read, a path
 * another file of the change already writes, or text that isn't JSON.
 */
export type DraftFileProblem = 'path' | 'duplicate' | 'json';

/** What stops a draft going, if anything. */
export type DraftCheck = {
  title?: 'blank' | 'tooLong';
  description?: 'tooLong';
  /** Each file's problem, in the draft's order. */
  files: (DraftFileProblem | undefined)[];
  /** Whether it can be sent: it writes a file and nothing stops it. */
  ready: boolean;
};

function bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Whether `path` names a file of the Definition a change may write. */
export function isDefinitionFile(path: string): boolean {
  return (
    (DEFINITION_FILES as readonly string[]).includes(path) ||
    FLOW_FILE.test(path)
  );
}

function isJson(text: string): boolean {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * Checks a draft as far as the browser can: a title, a path the
 * Definition reads for each file, once each, and JSON text. Whether the
 * Definition reads with the change is the factory's to say.
 */
export function checkDraft(draft: DefinitionChangeDraft): DraftCheck {
  const title = draft.title.trim();
  const titleProblem =
    title.length === 0
      ? 'blank'
      : bytes(title) > MAX_CHANGE_TITLE_BYTES
        ? 'tooLong'
        : undefined;
  const descriptionProblem =
    bytes(draft.description.trim()) > MAX_CHANGE_DESCRIPTION_BYTES
      ? 'tooLong'
      : undefined;

  const seen = new Set<string>();
  const files = draft.files.map((file): DraftFileProblem | undefined => {
    const path = file.path.trim();
    if (!isDefinitionFile(path)) return 'path';
    if (seen.has(path)) return 'duplicate';
    seen.add(path);
    return isJson(file.text) ? undefined : 'json';
  });

  return {
    title: titleProblem,
    description: descriptionProblem,
    files,
    ready:
      draft.files.length > 0 &&
      !titleProblem &&
      !descriptionProblem &&
      files.every((problem) => problem === undefined),
  };
}

/** The request a checked draft sends, under the change's id. */
export function changeRequest(
  draft: DefinitionChangeDraft,
  changeId: string
): ProposeDefinitionChangeRequest {
  const description = draft.description.trim();
  return {
    changeId,
    title: draft.title.trim(),
    description: description.length > 0 ? description : null,
    files: Object.fromEntries(
      draft.files.map((file) => [file.path.trim(), file.text])
    ),
  };
}

/**
 * A file's page on GitHub at the commit the change goes on top of, or at
 * the default branch's head when that isn't known.
 */
export function definitionFileUrl(
  repoUrl: string,
  path: string,
  commit?: string | null
): string {
  return `${repoUrl}/blob/${commit ?? 'HEAD'}/${path}`;
}
