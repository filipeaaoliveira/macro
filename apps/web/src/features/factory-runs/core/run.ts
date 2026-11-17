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

/** One Run of a Flow, with the Steps it took in the order they started. */
export type Run = {
  id: string;
  request: string;
  createdAt: string;
  taskId?: string;
  repository?: RunRepository;
  end?: { outcome: RunOutcome; endedAt: string };
  steps: RunStep[];
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

/** The first line of the request, which names the Run in lists. */
export function runTitle(run: Run): string {
  return run.request.trim().split('\n', 1)[0] ?? '';
}

/** What the request says after the first line, which is the Run's title. */
export function runRequestBody(run: Run): string {
  const [, ...rest] = run.request.trim().split('\n');
  return rest.join('\n').trim();
}
