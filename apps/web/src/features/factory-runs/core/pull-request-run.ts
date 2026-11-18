import type { BoardRun } from './board';
import type { RunStep } from './run';

/**
 * The Run that opened a pull request, as the board shows it, with the Step
 * whose merge Gate it waits on, if it does.
 */
export type PullRequestRun = BoardRun & {
  /** The place of the Step whose merge Gate the Run waits on. */
  mergeGateSeq?: number;
};

/** Why merging a Run's pull request from Reviews does nothing now. */
export type MergeRefusal =
  /** Someone else's Run: only its owner answers its merge Gate. */
  | { why: 'notOwner'; ownerId?: string }
  /** Its base branch is under a Main Hold. */
  | { why: 'held'; branch: string }
  /** It is at another Step, or at another Gate: no merge Gate is open. */
  | { why: 'notAtGate'; step?: RunStep };

/** How merging a pull request from Reviews goes. */
export type PullRequestMerge =
  /** No Run guards it: GitHub merges it as any pull request. */
  | { kind: 'github' }
  /** Its Run waits on its owner, the viewer, at its merge Gate. */
  | { kind: 'gate'; runId: string }
  /** Its Run guards it and takes no merge now. */
  | ({ kind: 'refused' } & MergeRefusal);

/**
 * How merging the pull request `found` opened goes for `viewerId`. While
 * its Run goes, a merge is its merge Gate's answer, which only its owner
 * gives, and only while the Run waits there; once the Run ended, the pull
 * request is the person's to merge as any other.
 */
export function pullRequestMerge(
  found: PullRequestRun | undefined,
  viewerId: string | undefined
): PullRequestMerge {
  if (!found || found.run.end) return { kind: 'github' };
  const run = found.run;
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
  return { kind: 'refused', why: 'notAtGate', step: run.steps.at(-1) };
}

/** The Run's latest review round's number, if it reviewed. */
export function latestReviewRound(found: PullRequestRun): number | undefined {
  return found.run.reviewRounds?.at(-1)?.round;
}
