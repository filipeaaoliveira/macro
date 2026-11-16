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

/** One visit of one of a Flow's entries. */
export type RunStep = {
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
};

/** One Run of a Flow, with the Steps it took in the order they started. */
export type Run = {
  id: string;
  request: string;
  createdAt: string;
  taskId?: string;
  end?: { outcome: RunOutcome; endedAt: string };
  steps: RunStep[];
};

/** A Flow a person can start a Run of. */
export type Flow = {
  name: string;
  description: string;
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
