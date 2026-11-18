import type { Accessor } from 'solid-js';
import { createContext, useContext } from 'solid-js';
import type { Flow, Run, RunStep } from '../core/run';

/**
 * Server data a view shows. `error` means nothing could be loaded: data a
 * refetch failed to replace stays `ready`.
 */
export type RunsSource<T> = {
  state: Accessor<'pending' | 'error' | 'ready'>;
  data: Accessor<T | undefined>;
  retry: () => void;
};

/**
 * How a write went. `conflict` means the server refused it because the Run
 * moved on: it ended, or someone else answered first.
 */
export type RunActionResult<T = undefined> =
  | { ok: true; value: T }
  | { ok: false; reason: 'conflict' | 'failed' };

/**
 * Why a Run didn't start. `unreachableRepository` and `noPushAccess` mean the
 * person's linked GitHub account can't reach the repository, or can't push to
 * it.
 */
export type StartRunFailure =
  | 'failed'
  | 'unreachableRepository'
  | 'noPushAccess'
  | 'queueFull';

/**
 * The person's Runs that hold a place in their Queue, newest first: those
 * waiting on them at a Gate, and those of a Flow with a Gate that have not
 * reached one yet.
 */
export type RunsQueue = {
  waiting: Run[];
  starting: Run[];
};

export type StartRunResult =
  | { ok: true; value: Run }
  | { ok: false; reason: StartRunFailure };

export type StartRunInput = {
  taskId: string;
  flow: string;
  request: string;
  /** The repository a Flow that works on one works on, by its URL. */
  repoUrl?: string;
};

export type AnswerGateInput = {
  runId: string;
  step: Pick<RunStep, 'seq'>;
  answer: string;
  note?: string;
  /** The Run's Tampered files the person saw and approves with the answer. */
  approvedControlFiles?: string[];
};

export type ReplyToGateInput = {
  runId: string;
  step: Pick<RunStep, 'seq'>;
  /** The person's words, in place of one of the Gate's answers. */
  reply: string;
};

/** What the Run's owner does to a running Step from its Step Terminal. */
export type SteerStepInput = {
  runId: string;
  step: Pick<RunStep, 'seq'>;
} & (
  | { action: 'note'; note: string }
  | { action: 'resume'; note?: string }
  | { action: 'pause' | 'stop' }
);

/** What the Runs views need from the factory. */
export type RunsContext = {
  /** The task's Runs, newest first; kept current while any of them runs. */
  taskRuns: (taskId: Accessor<string>) => RunsSource<Run[]>;
  /** One Run; kept current until it ends. */
  run: (runId: Accessor<string>) => RunsSource<Run>;
  /** The person's Runs that hold a place in their Queue. */
  waitingRuns: () => RunsSource<RunsQueue>;
  flows: () => RunsSource<Flow[]>;
  /**
   * The URLs of the GitHub repositories the person can start a Run on, read
   * while `enabled`.
   */
  repositories: (enabled: Accessor<boolean>) => RunsSource<string[]>;
  startRun: (input: StartRunInput) => Promise<StartRunResult>;
  answerGate: (input: AnswerGateInput) => Promise<RunActionResult>;
  /**
   * Gives the Gate the person's words, which the factory reads as one of its
   * answers or asks back on.
   */
  replyToGate: (input: ReplyToGateInput) => Promise<RunActionResult>;
  cancelRun: (runId: string) => Promise<RunActionResult>;
  /**
   * Sends a running Step's agent a note, pauses, resumes or stops the Step.
   * `conflict` means the Step takes no such steering now: it ended, is
   * paused (for a note), or its agent works on no turn.
   */
  steerStep: (input: SteerStepInput) => Promise<RunActionResult>;
};

const Context = createContext<RunsContext>();

export const RunsProvider = Context.Provider;

export function useRunsContext(): RunsContext {
  const context = useContext(Context);
  if (!context) throw new Error('RunsProvider is required');
  return context;
}
