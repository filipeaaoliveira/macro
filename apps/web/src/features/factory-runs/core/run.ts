/** How a Run ended, in the factory's words. */
export type RunOutcome =
  | 'success'
  | 'manual'
  | 'blocked'
  | 'failed'
  | 'cancelled';

/** A Standing Rule: an answer the factory gives a kind of Gate on its own. */
export type StandingRule = 'approve_plan' | 'fix_blockers' | 'merge_when_green';

/** The answer to a Gate: a person's, or a Standing Rule's (a Rule Answer). */
export type GateAnswer = {
  answer: string;
  note?: string;
  /** The Standing Rule that answered, for a Rule Answer. */
  rule?: StandingRule;
  /** The Run's Tampered files the answer approved. */
  approvedControlFiles?: string[];
  answeredAt: string;
};

/**
 * Who asks a Gate's question: the Flow, once its Step is done, offering
 * answers to pick from, or the Step's agent, which the Run's owner answers
 * in their own words (the note of its one answer, `answer`).
 */
export type GateKind = 'flow' | 'question';

/** A question a Step's end put to a person. */
export type Gate = {
  kind: GateKind;
  question: string;
  /** The answers it offers, in the Flow's order. */
  answers: string[];
  openedAt: string;
  answer?: GateAnswer;
};

/**
 * One round of the repository's checks a Step ran on its agent's work:
 * which checks, who ran them, and how they went (`passed` or `failed`, with
 * a report).
 */
export type CheckRound = {
  /** Which round, from 1. */
  round: number;
  /** The checks' names, in the repository's order. */
  checks: string[];
  startedAt: string;
  /**
   * Whether the factory ran the checks itself, on the pushed commit, and
   * read their exit codes; otherwise the agent ran them and reported how
   * they went.
   */
  verified: boolean;
  /** The exit code of each check a verified round ran so far. */
  results: CheckResult[];
  /** The commit a verified round's checks ran on: the pushed branch's head. */
  checkedCommit?: string;
  end?: {
    outcome: 'passed' | 'failed';
    /**
     * What each check did, as markdown: the agent's report, or the checks
     * that failed, with the end of their output.
     */
    report: string;
    endedAt: string;
  };
};

/** How one check of a verified round ran. */
export type CheckResult = {
  name: string;
  exitCode: number;
};

/** One visit of one of a Flow's entries. */
export type RunStep = {
  /** The Step's place among the Run's Steps, from 1: what keys it. */
  seq: number;
  entry: string;
  /** Which visit of the entry it is, from 1. */
  visit: number;
  startedAt: string;
  end?: {
    /** The Flow's word for how it ended (`done`, `blockers`, …). */
    outcome: string;
    /** What the Step produced, as markdown. */
    output: string;
    endedAt: string;
  };
  gate?: Gate;
  /** The Role an agent filled for the Step, as the Flow names it. */
  role?: string;
  /** The Agent that took the Step, as the agent definition names it. */
  agent?: string;
  /** The Agents before it that couldn’t start the Step, in order. */
  passedOver?: PassedOver[];
  /** The rounds of checks it ran, in order; none for most Steps. */
  checks?: CheckRound[];
};

/**
 * What a reviewer found: where, what is wrong and why. Once its round ends
 * it is a `blocker`, in the diff the round reviewed, which the fix gets, or
 * a `follow_up`, outside it, which became a task of its own.
 */
export type ReviewFinding = {
  /** The reviewer's Step. */
  seq: number;
  place: number;
  /** The reviewer's Role, as the Flow names it. */
  role?: string;
  path: string;
  lines?: { start: number; end: number };
  problem: string;
  why: string;
  /** A blocker of the round before found again, or an answer that never
   * read: it blocks wherever it is. */
  blocksAnywhere: boolean;
  /** Unset while its round reviews. */
  kind?: 'blocker' | 'follow_up';
  /** The task a follow-up became, when it was made. */
  followUpTaskId?: string;
};

/**
 * One round of a review panel: its reviewers review at once, each in a
 * Step of its own from `seq`, the Run's whole change in the first round and
 * the fix diff, what the fix changed since, in a later one.
 */
export type ReviewRound = {
  /** Which round of the Run, from 1, across every round it recorded. */
  round: number;
  /**
   * Which round of its entry since the entry was last reset, from 1: a
   * round after a reset (as when the change is sent back) is 1 again, and
   * reviews the whole change afresh.
   */
  visit: number;
  /** The place of its first Step. */
  seq: number;
  reviewers: number;
  /** Unset for a round that could not open. */
  headSha?: string;
  /** Where the diff under review starts: a branch, or a commit. */
  diffBase?: string;
  fixDiff: boolean;
  /** Why the round could not open, when it could not. */
  failure?: string;
  startedAt: string;
  end?: { outcome: 'pass' | 'changes' | 'failed'; endedAt: string };
  findings: ReviewFinding[];
};

/** An Agent that couldn’t start a Step, so the next one took it. */
export type PassedOver = {
  agent: string;
  /** Why, as a code such as `ai_allowance_exhausted`. */
  reason: string;
};

/** The GitHub repository a Run works on. */
export type RunRepository = {
  /** Its `https://github.com/owner/name` URL. */
  url: string;
  /** The branch the Run's work starts from and proposes its change to. */
  baseBranch: string;
};

/**
 * The Control Files a Run's branch changed: files that decide whether its
 * checks pass. `tampered` are the ones its request doesn't ask for, `named`
 * the ones it names. `unlisted` when GitHub couldn't list the branch's
 * changes, so which ones is unknown.
 */
export type ControlFileChanges = {
  tampered: string[];
  named: string[];
  unlisted: boolean;
};

/** Whether the Run merges only once a person approves its Control Files. */
export function isTampered(changes: ControlFileChanges): boolean {
  return changes.unlisted || changes.tampered.length > 0;
}

/**
 * The base branch a Run merges to is red: its required checks fail, so no
 * Run merges to it until they pass again.
 */
export type MainHold = {
  /** When the base was first seen red. */
  since: string;
  /** The head commit the checks fail on. */
  headSha: string;
  /** The failing checks, one line each. */
  failed: string;
  /** The task that names the merges since the base was last green. */
  taskId?: string;
};

/** Why a base branch's required checks hold no merge. */
export type BaseRequiredChecks = 'unread' | 'none';

/** One Run of a Flow, with the Steps it took in the order they started. */
export type Run = {
  id: string;
  request: string;
  createdAt: string;
  taskId?: string;
  repository?: RunRepository;
  /** The Control Files its branch changed, if any. */
  controlFiles?: ControlFileChanges;
  end?: { outcome: RunOutcome; endedAt: string };
  steps: RunStep[];
  /** While the Run is at its merge and its base branch is red. */
  mainHold?: MainHold;
  /**
   * While the Run is at its merge, why its base branch's required checks
   * hold nothing: they couldn't be read, or the branch requires none.
   */
  baseRequiredChecks?: BaseRequiredChecks;
  /** Its review rounds, in order; none for a Run with no review panel. */
  reviewRounds?: ReviewRound[];
};

/** A Flow a person can start a Run of. */
export type Flow = {
  name: string;
  description: string;
  /** A Run of it works on a GitHub repository, which starting it names. */
  repository: boolean;
};

/** Where a Run is now. */
export type RunStatus =
  /** A Step is running, or the next one is about to start. */
  | { kind: 'running'; step?: RunStep }
  /** The Run waits for a person to answer `step`'s Gate. */
  | { kind: 'waiting'; step: RunStep; gate: Gate }
  | { kind: 'ended'; outcome: RunOutcome; endedAt: string };

/**
 * The Run's status. A Run waits only on the Gate of its latest Step: an
 * earlier Gate was answered before the Run went on.
 */
export function runStatus(run: Run): RunStatus {
  if (run.end) {
    return {
      kind: 'ended',
      outcome: run.end.outcome,
      endedAt: run.end.endedAt,
    };
  }

  const step = run.steps.at(-1);
  if (step?.gate && !step.gate.answer) {
    return { kind: 'waiting', step, gate: step.gate };
  }

  return { kind: 'running', step };
}

/** The answer that agrees to a Gate; on a Tampered Run it needs approval. */
export const APPROVE_ANSWER = 'approve';

/** The one answer an agent's question offers: the owner's words go with it. */
export const QUESTION_ANSWER = 'answer';

/** Whether an agent asked `gate`'s question, so the owner writes the answer. */
export function isQuestion(gate: Pick<Gate, 'kind'>): boolean {
  return gate.kind === 'question';
}

/**
 * The Role of the agent that asks `gate`'s question, the Gate of `step`:
 * none for a question the Flow asks.
 */
export function gateAsker(
  step: Pick<RunStep, 'role'>,
  gate: Pick<Gate, 'kind'>
): string | undefined {
  return isQuestion(gate) ? (step.role ?? 'agent') : undefined;
}

/** The first line of the request, which names the Run in lists. */
export function runTitle(run: Run): string {
  return run.request.trim().split('\n', 1)[0] ?? '';
}

/** What the request says after the first line, which is the Run's title. */
export function runRequestBody(run: Run): string {
  const [, ...rest] = run.request.trim().split('\n');
  return rest.join('\n').trim();
}

/** The path of a finding about the whole change rather than one file. */
export const WHOLE_CHANGE = '.';

/** The review round whose first Step is at `seq`, if one starts there. */
export function roundStartingAt(
  rounds: ReviewRound[],
  seq: number
): ReviewRound | undefined {
  return rounds.find((round) => round.seq === seq);
}

/**
 * Whether `round` reviews afresh after its entry was reset (as when the
 * change is sent back): the Run's round before it of the same entry, the
 * entry its first Step in `steps` is of, came at the same visit or a later
 * one. A round of a second panel entry is not a fresh review of the first.
 */
export function reviewsAfresh(
  rounds: ReviewRound[],
  steps: Pick<RunStep, 'seq' | 'entry'>[],
  round: ReviewRound
): boolean {
  const entryOf = (seq: number) =>
    steps.find((step) => step.seq === seq)?.entry;
  const entry = entryOf(round.seq);
  if (entry === undefined) return false;
  const before = rounds
    .filter((other) => other.seq < round.seq && entryOf(other.seq) === entry)
    .at(-1);
  return before !== undefined && before.visit >= round.visit;
}

/** What the reviewer of the Step at `seq` found, in its order. */
export function findingsOf(
  rounds: ReviewRound[],
  seq: number
): ReviewFinding[] {
  return rounds.flatMap((round) =>
    round.findings.filter((finding) => finding.seq === seq)
  );
}

/** How many of a round's findings block, and how many became follow-ups. */
export function findingCounts(round: ReviewRound): {
  blockers: number;
  followUps: number;
} {
  let blockers = 0;
  let followUps = 0;
  for (const finding of round.findings) {
    if (finding.kind === 'blocker') blockers += 1;
    if (finding.kind === 'follow_up') followUps += 1;
  }
  return { blockers, followUps };
}
