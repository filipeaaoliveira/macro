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

export type StartRunInput = {
  taskId: string;
  flow: string;
  request: string;
};

export type AnswerGateInput = {
  runId: string;
  step: Pick<RunStep, 'entry' | 'visit'>;
  answer: string;
  note?: string;
};

/** What the Runs views need from the factory. */
export type RunsContext = {
  /** The task's Runs, newest first; kept current while any of them runs. */
  taskRuns: (taskId: Accessor<string>) => RunsSource<Run[]>;
  /** One Run; kept current until it ends. */
  run: (runId: Accessor<string>) => RunsSource<Run>;
  flows: () => RunsSource<Flow[]>;
  startRun: (input: StartRunInput) => Promise<RunActionResult<Run>>;
  answerGate: (input: AnswerGateInput) => Promise<RunActionResult>;
  cancelRun: (runId: string) => Promise<RunActionResult>;
};

const Context = createContext<RunsContext>();

export const RunsProvider = Context.Provider;

export function useRunsContext(): RunsContext {
  const context = useContext(Context);
  if (!context) throw new Error('RunsProvider is required');
  return context;
}
