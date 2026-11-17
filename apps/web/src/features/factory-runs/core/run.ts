/** How a Run ended, in the factory's words. */
export type RunOutcome =
  | 'success'
  | 'manual'
  | 'blocked'
  | 'failed'
  | 'cancelled';

/** A person's answer to a Gate. */
export type GateAnswer = {
  answer: string;
  note?: string;
  /** The Run's Tampered files the answer approved. */
  approvedControlFiles?: string[];
  answeredAt: string;
};

/** A question a Step's end put to a person. */
export type Gate = {
  question: string;
  /** The answers it offers, in the Flow's order. */
  answers: string[];
  openedAt: string;
  answer?: GateAnswer;
};

/**
 * One round of the repository's checks a Step had its agent run: which
 * checks, and how they went (`passed` or `failed`, with the agent's report).
 */
export type CheckRound = {
  /** Which round, from 1. */
  round: number;
  /** The checks' names, in the repository's order. */
  checks: string[];
  startedAt: string;
  end?: {
    outcome: 'passed' | 'failed';
    /** What the agent reported of each check, as markdown. */
    report: string;
    endedAt: string;
  };
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

/** The first line of the request, which names the Run in lists. */
export function runTitle(run: Run): string {
  return run.request.trim().split('\n', 1)[0] ?? '';
}

/** What the request says after the first line, which is the Run's title. */
export function runRequestBody(run: Run): string {
  const [, ...rest] = run.request.trim().split('\n');
  return rest.join('\n').trim();
}
