import type { BoardRun } from './board';
import type { RunOutcome, RunStep } from './run';

/**
 * What is known of the Run behind a pull request: not yet (`unknown`),
 * that none the person can see opened it (`none`), or its Run.
 */
export type PullRequestLookup =
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'run'; found: PullRequestRun };

/**
 * The Run that opened a pull request, as the board shows it, with the Step
 * whose merge Gate it waits on, if it does.
 */
export type PullRequestRun = BoardRun & {
  /** The place of the Step whose merge Gate the Run waits on. */
  mergeGateSeq?: number;
};

/** Why merging a pull request does nothing now. */
export type MergeRefusal =
  /** Whether a Run guards it is not known yet. */
  | { why: 'checking' }
  /** Someone else's Run: only its owner answers its merge Gate. */
  | { why: 'notOwner'; ownerId?: string }
  /** Its base branch is under a Main Hold. */
  | { why: 'held'; branch: string }
  /**
   * The Run is at another Step, or waits on its owner at another Gate
   * (`waiting`): no merge Gate is open.
   */
  | { why: 'notAtGate'; step?: RunStep; waiting: boolean };

/** How merging a pull request goes. */
export type PullRequestMerge =
  /**
   * No Run guards it: GitHub merges it as any pull request. `stopped` when
   * its Run ended for a person, so the merge skips what the Run checked.
   */
  | { kind: 'github'; stopped?: RunOutcome }
  /** Its Run waits on its owner, the viewer, at its merge Gate. */
  | { kind: 'gate'; runId: string }
  /** Its Run guards it, or may, and takes no merge now. */
  | ({ kind: 'refused' } & MergeRefusal);

/**
 * How merging the pull request goes for `viewerId`, from what is known of
 * its Run. Until that is known, nothing merges. While its Run goes, a merge
 * is its merge Gate's answer, which only its owner gives, and only while
 * the Run waits there; once the Run ended, the pull request is the
 * person's to merge as any other.
 */
export function pullRequestMerge(
  lookup: PullRequestLookup,
  viewerId: string | undefined
): PullRequestMerge {
  if (lookup.kind === 'unknown') return { kind: 'refused', why: 'checking' };
  if (lookup.kind === 'none') return { kind: 'github' };
  const found = lookup.found;
  const run = found.run;
  if (run.end) {
    return found.state === 'stopped'
      ? { kind: 'github', stopped: run.end.outcome }
      : { kind: 'github' };
  }
  if (!viewerId || run.ownerId !== viewerId) {
    return { kind: 'refused', why: 'notOwner', ownerId: run.ownerId };
  }
  if (found.mergeGateSeq !== undefined) {
    return { kind: 'gate', runId: run.id };
  }
  if (found.state === 'held') {
    return {
      kind: 'refused',
      why: 'held',
      branch: run.repository?.baseBranch ?? '',
    };
  }
  const step = run.steps.at(-1);
  return {
    kind: 'refused',
    why: 'notAtGate',
    step,
    waiting: Boolean(step?.gate && !step.gate.answer),
  };
}

/** The Run's latest review round's number, if it reviewed. */
export function latestReviewRound(found: PullRequestRun): number | undefined {
  return found.run.reviewRounds?.at(-1)?.round;
}
