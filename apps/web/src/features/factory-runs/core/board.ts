import type { Run, RunStep } from './run';

/** Where a Run is, as the board groups it. */
export type BoardState =
  /** A Step runs, or the next one is about to start. */
  | 'running'
  /** It waits on its owner at a Gate: a Flow's or an agent's question. */
  | 'waiting'
  /** It is at its merge and its base branch is under a Main Hold. */
  | 'held'
  /** It ended for a person to take over: manual, blocked or failed. */
  | 'stopped'
  /** It ended as asked, or a person cancelled it. */
  | 'finished';

/** The board's states, in the order it shows them. */
export const BOARD_STATES: BoardState[] = [
  'waiting',
  'held',
  'running',
  'stopped',
  'finished',
];

/** Whose Runs the board lists: the person's own, or their team's. */
export type BoardScope = 'mine' | 'team';

/** What narrows the board. Left out, a filter lists every Run. */
export type BoardFilters = {
  scope: BoardScope;
  state?: BoardState;
  flow?: string;
  repoUrl?: string;
  projectId?: string;
  /** Words in the Run's request or its task's name. */
  search?: string;
};

/**
 * One Run on the board, with where it is and its task's name; keyed by the
 * Run's id, so a read again keeps the row.
 */
export type BoardRun = {
  id: string;
  state: BoardState;
  taskName?: string;
  run: Run;
};

/** Where the board's next page starts. */
export type BoardCursor = { createdAt: string; id: string };

/** A page of the board, newest first. */
export type BoardPage = { runs: BoardRun[]; next?: BoardCursor };

/** Who started a Run. */
export type StartedBy =
  /** Its owner, in their own signed-in session. */
  | 'person'
  /** An agent for its owner, such as the Orchestrator in chat. */
  | 'orchestrator'
  /** One of the team's Automations. */
  | 'automation';

/** Who started `run`. */
export function startedBy(
  run: Pick<Run, 'automation' | 'createdInPerson'>
): StartedBy {
  if (run.automation) return 'automation';
  return run.createdInPerson === false ? 'orchestrator' : 'person';
}

/** The Step a Run is at: its latest, while it has not ended. */
export function currentStep(run: Pick<Run, 'end' | 'steps'>) {
  return run.end ? undefined : run.steps.at(-1);
}

/**
 * The Blockers a Run's review left open: those of its latest round, once it
 * ended asking for changes and until a later round reviews the fix. None
 * once the Run ended.
 */
export function openBlockers(run: Pick<Run, 'end' | 'reviewRounds'>): number {
  if (run.end) return 0;
  const latest = run.reviewRounds?.at(-1);
  if (latest?.end?.outcome !== 'changes') return 0;
  return latest.findings.filter((finding) => finding.kind === 'blocker').length;
}

/** The time a Run's age counts from where it is: its Step's start, else its own. */
export function stepSince(run: Run, step: RunStep | undefined): string {
  return step?.gate && !step.gate.answer
    ? step.gate.openedAt
    : (step?.startedAt ?? run.createdAt);
}

/** `owner/name` of a repository's `https://github.com/owner/name` URL. */
export function repositoryName(url: string): string {
  return url.replace(/^https:\/\/github\.com\//, '');
}

/** Whether `filters` narrow the board past whose Runs it lists. */
export function activeFilterCount(filters: BoardFilters): number {
  return [
    filters.state,
    filters.flow,
    filters.repoUrl,
    filters.projectId,
    filters.search,
  ].filter((value) => value !== undefined && value !== '').length;
}
